use super::{InterpolationMode, PageDrawInfo, Renderer, TextureHandle};
use crate::image::cache::DecodedImage;
use crate::image::cache::PixelData;
use crate::state::BindingDirection;
use glow::*;
use glutin::context::PossiblyCurrentContext;
use glutin::surface::{GlSurface, Surface, WindowSurface};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DEFAULT_QUALITY, DIB_RGB_COLORS,
    DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, FW_BOLD, FW_NORMAL, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::core::w;

pub struct OpenGLRenderer {
    gl: Arc<glow::Context>,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    program: Program,
    vao: VertexArray,
    _vbo: Buffer,

    // Shader Uniforms
    u_color_matrix: UniformLocation,
    u_offset: UniformLocation,
    u_tex_y: UniformLocation,
    u_tex_cb: UniformLocation,
    u_tex_cr: UniformLocation,
    u_dest_rect: UniformLocation,
    u_window_size: UniformLocation,
    u_is_ycbcr: UniformLocation,
    u_ui_color: UniformLocation,
    u_is_ui: UniformLocation,
    u_interpolation_mode: UniformLocation,
    u_source_texture_size: UniformLocation,
    interpolation_mode: InterpolationMode,
    text_alignment: AtomicI32,
}

impl OpenGLRenderer {
    pub fn new(
        gl: Arc<glow::Context>,
        context: PossiblyCurrentContext,
        surface: Surface<WindowSurface>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            // ブレンディングの有効化
            gl.enable(BLEND);
            gl.blend_func(SRC_ALPHA, ONE_MINUS_SRC_ALPHA);

            // 色味が明るくなるのを防ぐため、sRGB変換を無効化（Direct2Dと合わせる）
            gl.disable(FRAMEBUFFER_SRGB);

            // テクスチャアライメントの設定 (幅が4の倍数でない場合のため)
            gl.pixel_store_i32(UNPACK_ALIGNMENT, 1);

            // Shader Source
            let vert_src = r#"#version 330 core
                layout (location = 0) in vec3 aPos;
                layout (location = 1) in vec2 aTexCoord;
                out vec2 TexCoord;
                uniform vec4 uDestRect; // [left, top, right, bottom]
                uniform vec2 uWindowSize;
                void main() {
                    // NDC 変換: [0, w] -> [-1, 1], [0, h] -> [1, -1]
                    float x_coord = mix(uDestRect.x, uDestRect.z, aPos.x * 0.5 + 0.5);
                    float y_coord = mix(uDestRect.y, uDestRect.w, 0.5 - aPos.y * 0.5);
                    
                    float x_ndc = (x_coord / max(uWindowSize.x, 1.0)) * 2.0 - 1.0;
                    float y_ndc = 1.0 - (y_coord / max(uWindowSize.y, 1.0)) * 2.0;
                    
                    gl_Position = vec4(x_ndc, y_ndc, 0.0, 1.0);
                    TexCoord = aTexCoord;
                }
            "#;

            let frag_src = r#"#version 330 core
                out vec4 FragColor;
                in vec2 TexCoord;
                uniform sampler2D texY;
                uniform sampler2D texCb;
                uniform sampler2D texCr;
                uniform mat4 colorMatrix;
                uniform vec4 offset;
                uniform int isYCbCr; // bool ではなく int を使用 (互換性のため)
                uniform int isUI;
                uniform vec4 uiColor;
                uniform int interpolationMode; // 0=Nearest, 1=Linear, 2=Cubic, 3=Lanczos
                uniform vec2 sourceTextureSize;

                const float PI = 3.14159265359;

                // Cubic (Catmull-Rom) weight function
                float cubic_weight(float x) {
                    x = abs(x);
                    float x2 = x * x;
                    float x3 = x2 * x;
                    if (x <= 1.0) {
                        return 1.5 * x3 - 2.5 * x2 + 1.0;
                    } else if (x <= 2.0) {
                        return -0.5 * x3 + 2.5 * x2 - 4.0 * x + 2.0;
                    }
                    return 0.0;
                }

                // Lanczos weight function (a=3)
                float lanczos_weight(float x) {
                    if (x == 0.0) return 1.0;
                    x = abs(x);
                    if (x < 3.0) {
                        float pix = PI * x;
                        return sin(pix) * sin(pix / 3.0) / (pix * pix / 3.0);
                    }
                    return 0.0;
                }

                // サンプル関数 (YCbCr or RGBA に応じて分岐)
                vec4 sampleTexture(vec2 uv) {
                    if (isYCbCr != 0) {
                        float y = texture(texY, uv).r;
                        float cb = texture(texCb, uv).r;
                        float cr = texture(texCr, uv).r;
                        vec4 ycbcr = vec4(y, cb, cr, 1.0);
                        ycbcr = ycbcr + offset;
                        vec4 rgba = colorMatrix * ycbcr;
                        rgba.a = 1.0;
                        return clamp(rgba, 0.0, 1.0);
                    } else {
                        return texture(texY, uv);
                    }
                }

                // Cubic 補間 (4x4 サンプリング)
                vec4 sampleCubic(vec2 uv) {
                    vec2 texelSize = 1.0 / sourceTextureSize;
                    vec2 pixelPos = uv * sourceTextureSize - 0.5;
                    vec2 fracPart = fract(pixelPos);
                    vec2 basePos = (floor(pixelPos) + 0.5) * texelSize;

                    vec4 color = vec4(0.0);
                    float totalWeight = 0.0;

                    for (int j = -1; j <= 2; j++) {
                        for (int i = -1; i <= 2; i++) {
                            vec2 sampleUV = basePos + vec2(float(i), float(j)) * texelSize;
                            sampleUV = clamp(sampleUV, vec2(0.0), vec2(1.0));
                            
                            float wx = cubic_weight(float(i) - fracPart.x);
                            float wy = cubic_weight(float(j) - fracPart.y);
                            float w = wx * wy;
                            
                            color += sampleTexture(sampleUV) * w;
                            totalWeight += w;
                        }
                    }
                    return color / max(totalWeight, 0.001);
                }

                // Lanczos3 補間 (6x6 サンプリング)
                vec4 sampleLanczos(vec2 uv) {
                    vec2 texelSize = 1.0 / sourceTextureSize;
                    vec2 pixelPos = uv * sourceTextureSize - 0.5;
                    vec2 fracPart = fract(pixelPos);
                    vec2 basePos = (floor(pixelPos) + 0.5) * texelSize;

                    vec4 color = vec4(0.0);
                    float totalWeight = 0.0;

                    for (int j = -2; j <= 3; j++) {
                        for (int i = -2; i <= 3; i++) {
                            vec2 sampleUV = basePos + vec2(float(i), float(j)) * texelSize;
                            sampleUV = clamp(sampleUV, vec2(0.0), vec2(1.0));
                            
                            float wx = lanczos_weight(float(i) - fracPart.x);
                            float wy = lanczos_weight(float(j) - fracPart.y);
                            float w = wx * wy;
                            
                            color += sampleTexture(sampleUV) * w;
                            totalWeight += w;
                        }
                    }
                    return color / max(totalWeight, 0.001);
                }

                void main() {
                    if (isUI != 0) {
                        FragColor = uiColor;
                        return;
                    }
                    
                    // 補間モードに応じてサンプリング
                    if (interpolationMode == 3) {
                        // Lanczos3
                        FragColor = sampleLanczos(TexCoord);
                    } else if (interpolationMode == 2) {
                        // Cubic
                        FragColor = sampleCubic(TexCoord);
                    } else {
                        // Nearest (0) / Linear (1) - ハードウェアサンプラーに任せる
                        FragColor = sampleTexture(TexCoord);
                    }
                }
            "#;

            let program = gl.create_program()?;
            let vs = gl.create_shader(VERTEX_SHADER)?;
            gl.shader_source(vs, vert_src);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                return Err(format!("VS Compile Error: {}", gl.get_shader_info_log(vs)).into());
            }

            let fs = gl.create_shader(FRAGMENT_SHADER)?;
            gl.shader_source(fs, frag_src);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                return Err(format!("FS Compile Error: {}", gl.get_shader_info_log(fs)).into());
            }

            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(
                    format!("Program Link Error: {}", gl.get_program_info_log(program)).into(),
                );
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);

            let u_color_matrix = gl
                .get_uniform_location(program, "colorMatrix")
                .ok_or("Uniform colorMatrix not found")?;
            let u_offset = gl
                .get_uniform_location(program, "offset")
                .ok_or("Uniform offset not found")?;
            let u_tex_y = gl
                .get_uniform_location(program, "texY")
                .ok_or("Uniform texY not found")?;
            let u_tex_cb = gl
                .get_uniform_location(program, "texCb")
                .ok_or("Uniform texCb not found")?;
            let u_tex_cr = gl
                .get_uniform_location(program, "texCr")
                .ok_or("Uniform texCr not found")?;
            let u_dest_rect = gl
                .get_uniform_location(program, "uDestRect")
                .ok_or("Uniform uDestRect not found")?;
            let u_window_size = gl
                .get_uniform_location(program, "uWindowSize")
                .ok_or("Uniform uWindowSize not found")?;
            let u_is_ycbcr = gl
                .get_uniform_location(program, "isYCbCr")
                .ok_or("Uniform isYCbCr not found")?;
            let u_is_ui = gl
                .get_uniform_location(program, "isUI")
                .ok_or("Uniform isUI not found")?;
            let u_ui_color = gl
                .get_uniform_location(program, "uiColor")
                .ok_or("Uniform uiColor not found")?;
            let u_interpolation_mode = gl
                .get_uniform_location(program, "interpolationMode")
                .ok_or("Uniform interpolationMode not found")?;
            let u_source_texture_size = gl
                .get_uniform_location(program, "sourceTextureSize")
                .ok_or("Uniform sourceTextureSize not found")?;

            // Quad Setup
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));

            let vbo = gl.create_buffer()?;
            gl.bind_buffer(ARRAY_BUFFER, Some(vbo));

            let vertices: [f32; 30] = [
                -1.0, 1.0, 0.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 1.0,
                -1.0, 1.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0,
            ];
            gl.buffer_data_u8_slice(ARRAY_BUFFER, bytemuck::cast_slice(&vertices), STATIC_DRAW);
            gl.vertex_attrib_pointer_f32(0, 3, FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 2, FLOAT, false, 20, 12);
            gl.enable_vertex_attrib_array(1);

            Ok(Self {
                gl,
                context,
                surface,
                program,
                vao,
                _vbo: vbo,
                u_color_matrix,
                u_offset,
                u_tex_y,
                u_tex_cb,
                u_tex_cr,
                u_dest_rect,
                u_window_size,
                u_is_ycbcr,
                u_is_ui,
                u_ui_color,
                u_interpolation_mode,
                u_source_texture_size,
                interpolation_mode: InterpolationMode::Linear,
                text_alignment: AtomicI32::new(DWRITE_TEXT_ALIGNMENT_LEADING.0),
            })
        }
    }

    fn create_texture_f32(
        &self,
        width: u32,
        height: u32,
        data: &[f32],
    ) -> Result<Texture, Box<dyn std::error::Error>> {
        unsafe {
            let tex = self.gl.create_texture()?;
            self.gl.bind_texture(TEXTURE_2D, Some(tex));
            self.gl.tex_image_2d(
                TEXTURE_2D,
                0,
                R32F as i32,
                width as i32,
                height as i32,
                0,
                RED,
                FLOAT,
                Some(bytemuck::cast_slice(data)),
            );
            let filter = match self.interpolation_mode {
                InterpolationMode::NearestNeighbor => NEAREST as i32,
                _ => LINEAR as i32,
            };
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_MIN_FILTER, filter);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_MAG_FILTER, filter);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32);
            Ok(tex)
        }
    }

    fn create_texture_rgba8(
        &self,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<Texture, Box<dyn std::error::Error>> {
        unsafe {
            let tex = self.gl.create_texture()?;
            self.gl.bind_texture(TEXTURE_2D, Some(tex));
            self.gl.tex_image_2d(
                TEXTURE_2D,
                0,
                RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                RGBA,
                UNSIGNED_BYTE,
                Some(data),
            );
            let filter = match self.interpolation_mode {
                InterpolationMode::NearestNeighbor => NEAREST as i32,
                _ => LINEAR as i32,
            };
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_MIN_FILTER, filter);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_MAG_FILTER, filter);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32);
            self.gl
                .tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32);
            Ok(tex)
        }
    }
}

unsafe impl Send for OpenGLRenderer {}
unsafe impl Sync for OpenGLRenderer {}

impl Renderer for OpenGLRenderer {
    fn resize(&self, width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            self.surface.resize(
                &self.context,
                std::num::NonZeroU32::new(width).unwrap(),
                std::num::NonZeroU32::new(height).unwrap(),
            );
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    fn begin_draw(&self) {
        unsafe {
            let sw = self.surface.width().map(|v| v as i32).unwrap_or(0);
            let sh = self.surface.height().map(|v| v as i32).unwrap_or(0);
            self.gl.viewport(0, 0, sw, sh);
            self.gl.clear_color(0.1, 0.1, 0.1, 1.0);
            self.gl.clear(COLOR_BUFFER_BIT);
        }
    }

    fn end_draw(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.surface.swap_buffers(&self.context)?;
        Ok(())
    }

    fn upload_image(
        &self,
        image: &DecodedImage,
    ) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>> {
        match &image.pixel_data {
            PixelData::Rgba8(data) => {
                let tex = self.create_texture_rgba8(image.width, image.height, data)?;
                Ok(TextureHandle::OpenGL {
                    id: unsafe { std::mem::transmute_copy::<Texture, u32>(&tex) },
                    width: image.width,
                    height: image.height,
                })
            }
            PixelData::Ycbcr {
                planes,
                subsampling,
                precision,
                y_is_signed,
                c_is_signed,
            } => {
                let max_val = ((1u32 << *precision) - 1) as f32;
                let scale_val = 1.0 / max_val;
                let y_f32: Vec<f32> = planes[0].iter().map(|&v| v as f32 * scale_val).collect();
                let y_tex = self.create_texture_f32(image.width, image.height, &y_f32)?;
                let (dx, dy) = *subsampling;
                let c_width = (image.width + dx as u32 - 1) / dx as u32;
                let c_height = (image.height + dy as u32 - 1) / dy as u32;
                let cb_f32: Vec<f32> = planes[1].iter().map(|&v| v as f32 * scale_val).collect();
                let cr_f32: Vec<f32> = planes[2].iter().map(|&v| v as f32 * scale_val).collect();
                let cb_tex = self.create_texture_f32(c_width, c_height, &cb_f32)?;
                let cr_tex = self.create_texture_f32(c_width, c_height, &cr_f32)?;
                unsafe {
                    Ok(TextureHandle::OpenGLYCbCr {
                        y: std::mem::transmute_copy::<Texture, u32>(&y_tex),
                        cb: std::mem::transmute_copy::<Texture, u32>(&cb_tex),
                        cr: std::mem::transmute_copy::<Texture, u32>(&cr_tex),
                        width: image.width,
                        height: image.height,
                        _subsampling: *subsampling,
                        _precision: *precision,
                        y_is_signed: *y_is_signed,
                        c_is_signed: *c_is_signed,
                    })
                }
            }
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        unsafe {
            self.gl.use_program(Some(self.program));
            self.gl.uniform_1_i32(Some(&self.u_is_ui), 0);

            let sw = self.surface.width().map(|v| v as f32).unwrap_or(1.0);
            let sh = self.surface.height().map(|v| v as f32).unwrap_or(1.0);
            self.gl.uniform_2_f32(Some(&self.u_window_size), sw, sh);
            self.gl.uniform_4_f32(
                Some(&self.u_dest_rect),
                dest_rect.left,
                dest_rect.top,
                dest_rect.right,
                dest_rect.bottom,
            );

            // 補間モードをシェーダーに渡す
            let mode_int = match self.interpolation_mode {
                InterpolationMode::NearestNeighbor => 0,
                InterpolationMode::Linear => 1,
                InterpolationMode::Cubic => 2,
                InterpolationMode::Lanczos => 3,
            };
            self.gl
                .uniform_1_i32(Some(&self.u_interpolation_mode), mode_int);

            match texture {
                TextureHandle::OpenGL { id, width, height } => {
                    self.gl.uniform_1_i32(Some(&self.u_is_ycbcr), 0);
                    self.gl.uniform_2_f32(
                        Some(&self.u_source_texture_size),
                        *width as f32,
                        *height as f32,
                    );
                    self.gl.active_texture(TEXTURE0);
                    let tex: Texture = std::mem::transmute_copy::<u32, Texture>(id);
                    self.gl.bind_texture(TEXTURE_2D, Some(tex));
                    self.gl.uniform_1_i32(Some(&self.u_tex_y), 0);
                }
                TextureHandle::OpenGLYCbCr {
                    y,
                    cb,
                    cr,
                    width,
                    height,
                    y_is_signed,
                    c_is_signed,
                    ..
                } => {
                    self.gl.uniform_1_i32(Some(&self.u_is_ycbcr), 1);
                    self.gl.uniform_2_f32(
                        Some(&self.u_source_texture_size),
                        *width as f32,
                        *height as f32,
                    );
                    let y_offset = if *y_is_signed { 0.5 } else { 0.0 };
                    let c_offset = if *c_is_signed { 0.0 } else { -0.5 };
                    let matrix = [
                        1.0, 1.0, 1.0, 0.0, 0.0, -0.344136, 1.772, 0.0, 1.402, -0.714136, 0.0, 0.0,
                        0.0, 0.0, 0.0, 1.0,
                    ];
                    self.gl
                        .uniform_matrix_4_f32_slice(Some(&self.u_color_matrix), false, &matrix);
                    self.gl
                        .uniform_4_f32(Some(&self.u_offset), y_offset, c_offset, c_offset, 0.0);

                    self.gl.active_texture(TEXTURE0);
                    self.gl.bind_texture(
                        TEXTURE_2D,
                        Some(std::mem::transmute_copy::<u32, Texture>(y)),
                    );
                    self.gl.uniform_1_i32(Some(&self.u_tex_y), 0);
                    self.gl.active_texture(TEXTURE1);
                    self.gl.bind_texture(
                        TEXTURE_2D,
                        Some(std::mem::transmute_copy::<u32, Texture>(cb)),
                    );
                    self.gl.uniform_1_i32(Some(&self.u_tex_cb), 1);
                    self.gl.active_texture(TEXTURE2);
                    self.gl.bind_texture(
                        TEXTURE_2D,
                        Some(std::mem::transmute_copy::<u32, Texture>(cr)),
                    );
                    self.gl.uniform_1_i32(Some(&self.u_tex_cr), 2);
                }
                _ => return,
            }
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(TRIANGLES, 0, 6);
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        match texture {
            TextureHandle::OpenGLYCbCr { width, height, .. } => (*width as f32, *height as f32),
            TextureHandle::OpenGL { width, height, .. } => (*width as f32, *height as f32),
            _ => (0.0, 0.0),
        }
    }

    fn fill_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F) {
        unsafe {
            self.gl.use_program(Some(self.program));
            self.gl.uniform_1_i32(Some(&self.u_is_ui), 1);
            self.gl
                .uniform_4_f32(Some(&self.u_ui_color), color.r, color.g, color.b, color.a);

            // UI 描画時はこれらの uniform は使われないが、初期化しておく
            self.gl.uniform_1_i32(Some(&self.u_interpolation_mode), 1); // Linear
            self.gl
                .uniform_2_f32(Some(&self.u_source_texture_size), 1.0, 1.0); // ゼロ除算防止

            let sw = self.surface.width().map(|v| v as f32).unwrap_or(1.0);
            let sh = self.surface.height().map(|v| v as f32).unwrap_or(1.0);
            self.gl.uniform_2_f32(Some(&self.u_window_size), sw, sh);
            self.gl.uniform_4_f32(
                Some(&self.u_dest_rect),
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
            );

            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.draw_arrays(TRIANGLES, 0, 6);
        }
    }

    fn draw_rectangle(&self, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, stroke_width: f32) {
        // Draw 4 lines to form a hollow rectangle
        // Top
        self.fill_rectangle(
            &D2D_RECT_F {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.top + stroke_width,
            },
            color,
        );
        // Bottom
        self.fill_rectangle(
            &D2D_RECT_F {
                left: rect.left,
                top: rect.bottom - stroke_width,
                right: rect.right,
                bottom: rect.bottom,
            },
            color,
        );
        // Left
        self.fill_rectangle(
            &D2D_RECT_F {
                left: rect.left,
                top: rect.top + stroke_width,
                right: rect.left + stroke_width,
                bottom: rect.bottom - stroke_width,
            },
            color,
        );
        // Right
        self.fill_rectangle(
            &D2D_RECT_F {
                left: rect.right - stroke_width,
                top: rect.top + stroke_width,
                right: rect.right,
                bottom: rect.bottom - stroke_width,
            },
            color,
        );
    }

    fn draw_text(&self, text: &str, rect: &D2D_RECT_F, color: &D2D1_COLOR_F, large: bool) {
        let width = (rect.right - rect.left).ceil() as i32;
        let height = (rect.bottom - rect.top).ceil() as i32;
        if width <= 0 || height <= 0 {
            return;
        }

        unsafe {
            let hdc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut p_bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbitmap =
                CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut p_bits, None, 0).unwrap();
            let old_bitmap = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(hbitmap.0));

            // Clear to transparent (0)
            std::ptr::write_bytes(p_bits, 0, (width * height * 4) as usize);

            let font_height = if large { 32 } else { 18 };
            let weight = if large { FW_BOLD } else { FW_NORMAL };
            let hfont = CreateFontW(
                font_height,
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Yu Gothic UI"),
            );
            let old_font = SelectObject(hdc, windows::Win32::Graphics::Gdi::HGDIOBJ(hfont.0));

            SetTextColor(hdc, COLORREF(0x00FFFFFF)); // White
            SetBkMode(hdc, TRANSPARENT);

            let mut wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let mut rect_gdi = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };

            let alignment = DWRITE_TEXT_ALIGNMENT(self.text_alignment.load(Ordering::Relaxed));
            let mut format = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;
            if alignment == DWRITE_TEXT_ALIGNMENT_CENTER {
                format |= DT_CENTER;
            } else if alignment == DWRITE_TEXT_ALIGNMENT_TRAILING {
                format |= DT_RIGHT;
            } else {
                format |= DT_LEFT;
            }

            DrawTextW(hdc, &mut wide_text, &mut rect_gdi, format);

            // Apply color and use luminance as alpha
            let r = (color.r * 255.0) as u8;
            let g = (color.g * 255.0) as u8;
            let b = (color.b * 255.0) as u8;

            let pixel_sl =
                std::slice::from_raw_parts_mut(p_bits as *mut u32, (width * height) as usize);

            for p in pixel_sl {
                // GDI uses BGRA (little endian) -> 0xAARRGGBB in u32 but in bytes it is BB GG RR AA
                // Text is white, so we can take any channel as intensity/alpha
                let intensity = (*p & 0xFF) as u8; // Blue channel
                if intensity > 0 {
                    // Pre-multiplied alpha or straight alpha? Glow/OpenGL blending is usually configured.
                    // Assuming gl.blend_func(SRC_ALPHA, ONE_MINUS_SRC_ALPHA) and non-premultiplied texture?
                    // Let's use straight alpha texture.
                    // u32 is 0xAABBGGRR in Little Endian (R at lowest byte)? No.
                    // 0xAABBGGRR on LE machine:
                    // Byte 0: RR
                    // Byte 1: GG
                    // Byte 2: BB
                    // Byte 3: AA
                    // We need to form this u32.
                    *p = ((intensity as u32) << 24)
                        | ((b as u32) << 16)
                        | ((g as u32) << 8)
                        | (r as u32);
                } else {
                    *p = 0;
                }
            }

            // Create texture
            let tex = self
                .create_texture_rgba8(
                    width as u32,
                    height as u32,
                    std::slice::from_raw_parts(p_bits as *const u8, (width * height * 4) as usize),
                )
                .unwrap();

            // Draw
            self.draw_image(
                &TextureHandle::OpenGL {
                    id: std::mem::transmute_copy::<Texture, u32>(&tex),
                    width: width as u32,
                    height: height as u32,
                },
                rect,
            );

            // Cleanup texture
            self.gl.delete_texture(tex);

            // GDI Cleanup
            let _ = SelectObject(hdc, old_font);
            let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(hfont.0));
            let _ = SelectObject(hdc, old_bitmap);
            let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(hbitmap.0));
            let _ = DeleteDC(hdc);
        }
    }

    fn set_interpolation_mode(&mut self, mode: InterpolationMode) {
        self.interpolation_mode = mode;
    }
    fn set_text_alignment(&self, alignment: DWRITE_TEXT_ALIGNMENT) {
        self.text_alignment.store(alignment.0, Ordering::Relaxed);
    }

    fn supports_page_turn_animation(&self) -> bool {
        true // OpenGLはページめくりアニメーションをサポート
    }

    fn draw_page_turn(
        &self,
        progress: f32,
        direction: i32,
        binding: BindingDirection,
        from_pages: &[PageDrawInfo],
        to_pages: &[PageDrawInfo],
        dest_rect: &D2D_RECT_F,
    ) {
        // シンプルなスライドアニメーション
        let width = dest_rect.right - dest_rect.left;
        let eased = 1.0 - (1.0 - progress).powi(3); // ease-out cubic

        let slide_direction = match (binding, direction) {
            (BindingDirection::Right, 1) => 1.0,
            (BindingDirection::Right, _) => -1.0,
            (BindingDirection::Left, 1) => -1.0,
            (BindingDirection::Left, _) => 1.0,
        };

        let offset = width * eased * slide_direction;

        // 遷移前（スライドアウト）
        for page in from_pages {
            let mut page_rect = page.dest_rect;
            page_rect.left += offset;
            page_rect.right += offset;

            if page_rect.right > 0.0 && page_rect.left < dest_rect.right + width {
                self.draw_image(page.texture, &page_rect);
            }
        }

        // 遷移後（スライドイン）
        let to_offset = offset - width * slide_direction;
        for page in to_pages {
            let mut page_rect = page.dest_rect;
            page_rect.left += to_offset;
            page_rect.right += to_offset;

            if page_rect.right > 0.0 && page_rect.left < dest_rect.right + width {
                self.draw_image(page.texture, &page_rect);
            }
        }
    }
}
