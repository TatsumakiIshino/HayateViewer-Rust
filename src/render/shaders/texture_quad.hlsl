// Simple texture quad shader for Direct3D 11
// Renders a textured quad with RGBA texture

cbuffer TransformParams : register(b0)
{
    float4 destRect;  // left, top, right, bottom (normalized)
    float4 srcRect;   // left, top, right, bottom (normalized, for texture atlas)
};

Texture2D<float4> texDiffuse : register(t0);
SamplerState samplerLinear : register(s0);

struct VSInput
{
    uint vertexId : SV_VertexID;
};

struct PSInput
{
    float4 position : SV_POSITION;
    float2 texCoord : TEXCOORD0;
};

// 頂点シェーダ（頂点バッファなしでクアッドを生成）
PSInput VSMain(VSInput input)
{
    PSInput output;
    
    // 頂点 ID から位置を計算 (0, 1, 2, 3 -> 四角形の4頂点)
    // 0: 左上, 1: 右上, 2: 左下, 3: 右下
    float2 pos;
    float2 uv;
    
    switch (input.vertexId)
    {
        case 0: // 左上
            pos = float2(destRect.x, destRect.y);
            uv = float2(srcRect.x, srcRect.y);
            break;
        case 1: // 右上
            pos = float2(destRect.z, destRect.y);
            uv = float2(srcRect.z, srcRect.y);
            break;
        case 2: // 左下
            pos = float2(destRect.x, destRect.w);
            uv = float2(srcRect.x, srcRect.w);
            break;
        case 3: // 右下
        default:
            pos = float2(destRect.z, destRect.w);
            uv = float2(srcRect.z, srcRect.w);
            break;
    }
    
    // ピクセル座標から NDC に変換 (0,0 左上, width,height 右下)
    // NDC: -1,-1 左下, 1,1 右上
    output.position = float4(pos.x * 2.0f - 1.0f, 1.0f - pos.y * 2.0f, 0.0f, 1.0f);
    output.texCoord = uv;
    
    return output;
}

// ピクセルシェーダ
float4 PSMain(PSInput input) : SV_TARGET
{
    return texDiffuse.Sample(samplerLinear, input.texCoord);
}
