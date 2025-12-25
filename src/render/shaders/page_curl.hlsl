cbuffer PageCurlConstants : register(b1)
{
    float progress;
    float direction; // 1.0: right-to-left, -1.0: left-to-right
    float viewport_width;
    float viewport_height;
    float4 dest_rect; // [left, top, right, bottom]
    float layer;
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
};

VS_OUTPUT VSMain(VS_INPUT input)
{
    VS_OUTPUT output;
    
    // input.pos is in [-1, 1] range mapping to destination rectangle
    float2 tc_in = input.pos.xy * 0.5 + 0.5;
    tc_in.y = 1.0 - tc_in.y; 
    
    float2 pixelPos = float2(
        lerp(dest_rect.x, dest_rect.z, tc_in.x),
        lerp(dest_rect.y, dest_rect.w, tc_in.y)
    );
    
    float pageWidth = dest_rect.z - dest_rect.x;
    float relX; 
    
    if (direction > 0.0) {
        // RTL: Spine is on Left
        relX = (pixelPos.x - dest_rect.x) / pageWidth;
    } else {
        // LTR: Spine is on Right
        relX = (dest_rect.z - pixelPos.x) / pageWidth;
    }

    float3 outPos = float3(pixelPos.x, pixelPos.y, 0.0);
    float shadow = 1.0;

    // We want the curl to travel more aggressively to ensure full completion.
    float r = pageWidth * (0.15 + progress * 0.1);
    float curlTrigger = 1.2 - progress * 2.4; // 1.2 to -1.2 relative to relX
    
    if (relX > curlTrigger) {
        float arcLen = (relX - curlTrigger) * pageWidth;
        float angle = arcLen / r;
        
        float dx = r * sin(angle);
        float dz = r * (1.0 - cos(angle));
        
        float basePos = curlTrigger * pageWidth;
        float activeX = basePos + dx;
        
        if (direction > 0.0) {
            outPos.x = dest_rect.x + activeX;
        } else {
            outPos.x = dest_rect.z - activeX;
        }
        outPos.z = dz;
        
        shadow = clamp(1.0 - dz / (r * 2.0) * 0.6, 0.4, 1.0);
        if (angle > 1.57) shadow *= 0.8;
        
        if (angle > 3.14159) {
            float backX = basePos - r * sin(angle);
            if (direction > 0.0) outPos.x = dest_rect.x + backX;
            else outPos.x = dest_rect.z - backX;
        }
    }

    // Convert back to NDC
    output.pos.x = (outPos.x / viewport_width) * 2.0 - 1.0;
    output.pos.y = 1.0 - (outPos.y / viewport_height) * 2.0;
    // D3D11 Depth range [0, 1]. Height(outPos.z) should make it come "forward" (smaller Z).
    output.pos.z = clamp(layer - (outPos.z / 10000.0), 0.0, 1.0); 
    output.pos.w = 1.0;
    
    output.tex = input.tex;
    output.shadow = shadow;
    
    return output;
}
