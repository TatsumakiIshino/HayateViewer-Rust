use std::sync::Arc;
use glow::*;
use glutin::context::PossiblyCurrentContext;
use glutin::surface::{Surface, WindowSurface, GlSurface};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT;
use super::{Renderer, TextureHandle, InterpolationMode};
use crate::image::cache::DecodedImage;
use crate::image::cache::PixelData;

pub struct OpenGLRenderer {
    gl: Arc<glow::Context>,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    program: Program,
    vao: VertexArray,
    vbo: Buffer,
    
    // Shader Uniforms
    u_color_matrix: UniformLocation,
    u_offset: UniformLocation,
    u_scale: UniformLocation,
    u_tex_y: UniformLocation,
    u_tex_cb: UniformLocation,
    u_tex_cr: UniformLocation,
    u_dest_rect: UniformLocation,
    u_window_size: UniformLocation,
}

impl OpenGLRenderer {
    pub fn new(
        gl: Arc<glow::Context>,
        context: PossiblyCurrentContext,
        surface: Surface<WindowSurface>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            // Shader Source
            let vert_src = r#"#version 330 core
                layout (location = 0) in vec3 aPos;
                layout (location = 1) in vec2 aTexCoord;
                out vec2 TexCoord;
                uniform vec4 uDestRect; // [left, top, right, bottom]
                uniform vec2 uWindowSize;
                void main() {
                    // NDC 変換: [0, w] -> [-1, 1], [0, h] -> [1, -1]
                    float x = mix(uDestRect.x, uDestRect.z, aPos.x * 0.5 + 0.5);
                    float y = mix(uDestRect.y, uDestRect.w, 0.5 - aPos.y * 0.5);
                    
                    float gl_x = (x / uWindowSize.x) * 2.0 - 1.0;
                    float gl_y = 1.0 - (y / uWindowSize.y) * 2.0;
                    
                    gl_Position = vec4(gl_x, gl_y, 0.0, 1.0);
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
                uniform vec4 scale;
                void main() {
                    float y = texture(texY, TexCoord).r;
                    float cb = texture(texCb, TexCoord).r;
                    float cr = texture(texCr, TexCoord).r;
                    
                    vec4 ycbcr = vec4(y, cb, cr, 1.0);
                    // 正規化済みデータ (0.0-1.0) に対するオフセット適用
                    ycbcr = ycbcr + offset;
                    
                    vec4 rgba = colorMatrix * ycbcr;
                    rgba.a = 1.0;
                    FragColor = clamp(rgba, 0.0, 1.0);
                }
            "#;

            let program = gl.create_program()?;
            let vs = gl.create_shader(VERTEX_SHADER)?;
            gl.shader_source(vs, vert_src);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                return Err(gl.get_shader_info_log(vs).into());
            }

            let fs = gl.create_shader(FRAGMENT_SHADER)?;
            gl.shader_source(fs, frag_src);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                return Err(gl.get_shader_info_log(fs).into());
            }

            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program).into());
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);

            let u_color_matrix = gl.get_uniform_location(program, "colorMatrix").ok_or("Uniform colorMatrix not found")?;
            let u_offset = gl.get_uniform_location(program, "offset").ok_or("Uniform offset not found")?;
            let u_scale = gl.get_uniform_location(program, "scale").ok_or("Uniform scale not found")?;
            let u_tex_y = gl.get_uniform_location(program, "texY").ok_or("Uniform texY not found")?;
            let u_tex_cb = gl.get_uniform_location(program, "texCb").ok_or("Uniform texCb not found")?;
            let u_tex_cr = gl.get_uniform_location(program, "texCr").ok_or("Uniform texCr not found")?;
            let u_dest_rect = gl.get_uniform_location(program, "uDestRect").ok_or("Uniform uDestRect not found")?;
            let u_window_size = gl.get_uniform_location(program, "uWindowSize").ok_or("Uniform uWindowSize not found")?;

            // Quad Setup
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(ARRAY_BUFFER, Some(vbo));
            
            // Full screen quad
            let vertices: [f32; 30] = [
                // Pos       // Tex
                -1.0,  1.0, 0.0,  0.0, 0.0,
                -1.0, -1.0, 0.0,  0.0, 1.0,
                 1.0, -1.0, 0.0,  1.0, 1.0,

                -1.0,  1.0, 0.0,  0.0, 0.0,
                 1.0, -1.0, 0.0,  1.0, 1.0,
                 1.0,  1.0, 0.0,  1.0, 0.0,
            ];
            
            gl.buffer_data_u8_slice(ARRAY_BUFFER, bytemuck::cast_slice(&vertices), STATIC_DRAW);
            
            gl.vertex_attrib_pointer_f32(0, 3, FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 2, FLOAT, false, 20, 12);
            gl.enable_vertex_attrib_array(1);

            Ok(Self {
                gl, context, surface, program, vao, vbo,
                u_color_matrix, u_offset, u_scale, u_tex_y, u_tex_cb, u_tex_cr,
                u_dest_rect, u_window_size,
            })
        }
    }

    fn create_texture(&self, width: u32, height: u32, data: &[f32]) -> Result<Texture, Box<dyn std::error::Error>> {
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
            
            self.gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR as i32);
            self.gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR as i32);
            self.gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32);
            self.gl.tex_parameter_i32(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32);
            
            Ok(tex)
        }
    }
}

// レンダリングループ(メインスレッド)でのみ使用されるため、Send/Sync を強制的に実装します。
unsafe impl Send for OpenGLRenderer {}
unsafe impl Sync for OpenGLRenderer {}

impl Renderer for OpenGLRenderer {
    fn resize(&self, width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            self.surface.resize(&self.context, 
                std::num::NonZeroU32::new(width).unwrap(), 
                std::num::NonZeroU32::new(height).unwrap()
            );
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    fn begin_draw(&self) {
        unsafe {
            self.gl.clear_color(0.1, 0.1, 0.1, 1.0);
            self.gl.clear(COLOR_BUFFER_BIT);
        }
    }

    fn end_draw(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.surface.swap_buffers(&self.context)?;
        Ok(())
    }

    fn upload_image(&self, image: &DecodedImage) -> std::result::Result<TextureHandle, Box<dyn std::error::Error>> {
        match &image.pixel_data {
            PixelData::Ycbcr { planes, subsampling, precision, y_is_signed, c_is_signed } => {
                if planes.len() != 3 { return Err("Invalid plane count".into()); }
                
                let max_val = ((1u32 << *precision) - 1) as f32;
                let scale_val = 1.0 / max_val;
                
                let y_f32: Vec<f32> = planes[0].iter().map(|&v| v as f32 * scale_val).collect();
                let y_tex = self.create_texture(image.width, image.height, &y_f32)?;
                
                let (dx, dy) = *subsampling;
                let c_width = (image.width + dx as u32 - 1) / dx as u32;
                let c_height = (image.height + dy as u32 - 1) / dy as u32;
                
                let cb_f32: Vec<f32> = planes[1].iter().map(|&v| v as f32 * scale_val).collect();
                let cr_f32: Vec<f32> = planes[2].iter().map(|&v| v as f32 * scale_val).collect();
                
                let cb_tex = self.create_texture(c_width, c_height, &cb_f32)?;
                let cr_tex = self.create_texture(c_width, c_height, &cr_f32)?;
                
                unsafe {
                    Ok(TextureHandle::OpenGLYCbCr {
                        y: std::mem::transmute(y_tex),
                        cb: std::mem::transmute(cb_tex),
                        cr: std::mem::transmute(cr_tex),
                        width: image.width,
                        height: image.height,
                        subsampling: *subsampling,
                        precision: *precision,
                        y_is_signed: *y_is_signed,
                        c_is_signed: *c_is_signed,
                    })
                }
            }
            _ => Err("Not implemented".into())
        }
    }

    fn draw_image(&self, texture: &TextureHandle, dest_rect: &D2D_RECT_F) {
        if let TextureHandle::OpenGLYCbCr { y, cb, cr, y_is_signed, c_is_signed, .. } = texture {
            unsafe {
                self.gl.use_program(Some(self.program));
                
                // 正規化済みデータに対するオフセット
                // Y コンポーネントが符号付きの場合は +0.5 して 0中心から 0.5中心 (0..1) に戻す
                let y_offset = if *y_is_signed { 0.5 } else { 0.0 };
                let c_offset = if *c_is_signed { 0.0 } else { -0.5 };
                
                let matrix = [
                    1.0, 1.0, 1.0, 0.0,
                    0.0, -0.344136, 1.772, 0.0,
                    1.402, -0.714136, 0.0, 0.0,
                    0.0, 0.0, 0.0, 1.0
                ];
                
                self.gl.uniform_matrix_4_f32_slice(Some(&self.u_color_matrix), false, &matrix);
                self.gl.uniform_4_f32(Some(&self.u_offset), y_offset, c_offset, c_offset, 0.0);
                self.gl.uniform_4_f32(Some(&self.u_scale), 1.0, 1.0, 1.0, 1.0);
                
                let sw = self.surface.width().map(|v| v as f32).unwrap_or(0.0);
                let sh = self.surface.height().map(|v| v as f32).unwrap_or(0.0);
                self.gl.uniform_2_f32(Some(&self.u_window_size), sw, sh);
                self.gl.uniform_4_f32(Some(&self.u_dest_rect), dest_rect.left, dest_rect.top, dest_rect.right, dest_rect.bottom);
 
                // Textures
                self.gl.active_texture(TEXTURE0);
                self.gl.bind_texture(TEXTURE_2D, Some(std::mem::transmute(*y)));
                self.gl.uniform_1_i32(Some(&self.u_tex_y), 0);
                
                self.gl.active_texture(TEXTURE1);
                self.gl.bind_texture(TEXTURE_2D, Some(std::mem::transmute(*cb)));
                self.gl.uniform_1_i32(Some(&self.u_tex_cb), 1);
                
                self.gl.active_texture(TEXTURE2);
                self.gl.bind_texture(TEXTURE_2D, Some(std::mem::transmute(*cr)));
                self.gl.uniform_1_i32(Some(&self.u_tex_cr), 2);

                self.gl.bind_vertex_array(Some(self.vao));
                self.gl.draw_arrays(TRIANGLES, 0, 6);
            }
        }
    }

    fn get_texture_size(&self, texture: &TextureHandle) -> (f32, f32) {
        if let TextureHandle::OpenGLYCbCr { width, height, .. } = texture {
            (*width as f32, *height as f32)
        } else {
            (0.0, 0.0)
        }
    }
    fn fill_rectangle(&self, _rect: &D2D_RECT_F, _color: &D2D1_COLOR_F) {}
    fn fill_rounded_rectangle(&self, _rect: &D2D_RECT_F, _radius: f32, _color: &D2D1_COLOR_F) {}
    fn draw_rectangle(&self, _rect: &D2D_RECT_F, _color: &D2D1_COLOR_F, _stroke_width: f32) {}
    fn draw_text(&self, _text: &str, _rect: &D2D_RECT_F, _color: &D2D1_COLOR_F, _large: bool) {}
    fn set_interpolation_mode(&mut self, _mode: InterpolationMode) {}
    fn set_text_alignment(&self, _alignment: DWRITE_TEXT_ALIGNMENT) {}
}
