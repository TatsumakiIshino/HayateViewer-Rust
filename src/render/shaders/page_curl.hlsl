cbuffer PageCurlConstants : register(b1)
{
    float progress;
    float direction; // 1.0: right-to-left (Right Binding Next), -1.0: left-to-right
    float viewport_width;
    float viewport_height;
    float4 dest_rect; // [left, top, right, bottom]
    float4 total_rect; // [left, top, right, bottom] of the whole spread
    float layer;
    float is_back_face; // 0.0 for front, 1.0 for back
    float2 _padding;
};

struct VS_INPUT
{
    float3 pos : POSITION;
    float2 tex : TEXCOORD;
};

struct VS_OUTPUT
{
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD;
    float shadow : COLOR;
    float isBackFace : TEXCOORD1;
};

VS_OUTPUT VSMain(VS_INPUT input)
{
    VS_OUTPUT output;
    
    // input.pos is in [-1, 1] mapping to dest_rect
    float2 tc_in = input.pos.xy * 0.5 + 0.5;
    tc_in.y = 1.0 - tc_in.y; 
    
    float2 pixelPos = float2(
        lerp(dest_rect.x, dest_rect.z, tc_in.x),
        lerp(dest_rect.y, dest_rect.w, tc_in.y)
    );
    
    float spineX = (total_rect.x + total_rect.z) * 0.5;
    float maxWidth = (total_rect.z - total_rect.x) * 0.5;
    if (maxWidth < 1.0) maxWidth = 1.0;

    float3 outPos = float3(pixelPos.x, pixelPos.y, 0.0);
    float shadow = 1.0;
    
    bool isTargetPage = false;
    float dist = 0.0;
    
    if (direction > 0.0) {
        if (pixelPos.x < spineX + 1.0) {
             isTargetPage = true;
             dist = spineX - pixelPos.x;
        }
    } else {
        if (pixelPos.x > spineX - 1.0) {
             isTargetPage = true;
             dist = pixelPos.x - spineX;
        }
    }

    float2 finalTex = input.tex;
    float theta = 0.0;

    if (isTargetPage && progress > 0.0) {
        // Curve parameters
        float PI = 3.14159;
        float p = progress;
        
        // 0 to pi rotation
        theta = p * PI;
        
        float r = dist;
        
        // Rotation around spine
        outPos.x = spineX + (direction > 0.0 ? -1.0 : 1.0) * r * cos(theta);
        outPos.z = r * sin(theta);
        
        // Slight curl at the edge
        float edgeCurl = pow(dist / maxWidth, 2.0) * p * 15.0;
        outPos.z += edgeCurl;

        // Shadow calculation: much brighter now
        if (theta > PI/2.0) {
            shadow = 0.98; // Back side is almost original brightness
            finalTex.x = 1.0 - finalTex.x;
        } else {
            shadow = 1.0;
        }
        
        // Soft gradient shadow depending on angle (darkest at 90 degrees)
        // Only darken as the page stands up
        float angleShadow = 1.0 - 0.2 * sin(theta); 
        shadow *= angleShadow;
        
        // Dynamic spine shadow: appears gradually as the page turns
        float spineShadowAmout = saturate(p * 2.0) * 0.15; // Max 15% darkening
        shadow *= lerp(1.0 - spineShadowAmout, 1.0, saturate(dist / 60.0));
    }

    output.pos.x = (outPos.x / viewport_width) * 2.0 - 1.0;
    output.pos.y = 1.0 - (outPos.y / viewport_height) * 2.0;
    output.pos.z = clamp(layer - (outPos.z / 1000.0), 0.0, 1.0); 
    output.pos.w = 1.0;
    
    output.tex = finalTex;
    output.shadow = shadow;
    output.isBackFace = (theta > 3.14159 / 2.0) ? 1.0 : 0.0;
    
    return output;
}

// Pixel Shader to handle front/back discard
Texture2D tex0 : register(t0);
SamplerState sam0 : register(s0);

float4 PSMain(VS_OUTPUT input) : SV_Target
{
    // Discard if we are drawing the wrong face
    if (is_back_face > 0.5) {
        if (input.isBackFace < 0.5) discard;
    } else {
        if (input.isBackFace > 0.5) discard;
    }

    float4 color = tex0.Sample(sam0, input.tex);
    color.rgb *= input.shadow;
    return color;
}

