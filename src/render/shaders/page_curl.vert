#version 330 core
layout (location = 0) in vec3 aPos;
layout (location = 1) in vec2 aTexCoord;
out vec2 TexCoord;
out float vShadow;

uniform vec4 uDestRect; // [left, top, right, bottom]
uniform vec2 uWindowSize;
uniform float progress;
uniform float direction; // 1.0: RTL (Spine on Left), -1.0: LTR (Spine on Right)
uniform float u_layer;   // 0.0: Back, 0.1: Front static, 0.2+: Curling

void main() {
    float pageWidth = uDestRect.z - uDestRect.x;
    
    // Normalize input [-1, 1] to [0, 1]
    vec2 tc_in = aPos.xy * 0.5 + 0.5;
    tc_in.y = 1.0 - tc_in.y; 
    
    // Initial pixel position inside world
    vec2 pixelPos = vec2(
        mix(uDestRect.x, uDestRect.z, tc_in.x),
        mix(uDestRect.y, uDestRect.w, tc_in.y)
    );
    
    float relX; 
    if (direction > 0.0) {
        relX = (pixelPos.x - uDestRect.x) / pageWidth;
    } else {
        relX = (uDestRect.z - pixelPos.x) / pageWidth;
    }

    vec3 outPos = vec3(pixelPos, 0.0);
    float shadow = 1.0;

    // Radius and Trigger logic
    // We want the curl to travel from relX=1 down to relX=0 and beyond.
    float r = pageWidth * (0.2 + progress * 0.1);
    float curlTrigger = 1.2 - progress * 2.4; // 1.2 -> -1.2
    
    if (relX > curlTrigger) {
        float arcLen = (relX - curlTrigger) * pageWidth;
        float angle = arcLen / r;
        
        // Deformation
        float dx = r * sin(angle);
        float dz = r * (1.0 - cos(angle));
        
        float basePos = curlTrigger * pageWidth;
        float activeX = basePos + dx;
        
        if (direction > 0.0) {
            outPos.x = uDestRect.x + activeX;
        } else {
            outPos.x = uDestRect.z - activeX;
        }
        outPos.z = dz;
        
        // Shadow based on height(dz) and angle
        shadow = clamp(1.0 - dz / (r * 2.0) * 0.6, 0.4, 1.0);
        if (angle > 1.57) shadow *= 0.8; // Backside is darker
        
        // Flip back if angle > PI
        if (angle > 3.14159) {
            float backX = basePos - r * sin(angle);
            if (direction > 0.0) outPos.x = uDestRect.x + backX;
            else outPos.x = uDestRect.z - backX;
        }
    }

    // Convert to NDC: [0, w] -> [-1, 1], [0, h] -> [1, -1]
    float x_ndc = (outPos.x / max(uWindowSize.x, 1.0)) * 2.0 - 1.0;
    float y_ndc = 1.0 - (outPos.y / max(uWindowSize.y, 1.0)) * 2.0;
    
    // Set Z based on layer and curl height
    gl_Position = vec4(x_ndc, y_ndc, u_layer - outPos.z * 0.0001, 1.0);
    TexCoord = aTexCoord;
    vShadow = shadow;
}
