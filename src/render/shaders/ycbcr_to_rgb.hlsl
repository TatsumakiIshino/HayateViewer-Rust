// YCbCr to RGB conversion shader for Direct3D 11
// Supports BT.601 and BT.709 color spaces
// Input: Y, Cb, Cr planes as separate textures (R32_SINT)
// Output: RGBA color
// 
// 補間モード対応: Nearest, Linear, Cubic (Catmull-Rom), Lanczos3

// 定数バッファ：変換行列とオフセット
cbuffer YCbCrParams : register(b0)
{
    float4x4 colorMatrix;  // YCbCr to RGB 変換行列
    float4 offset;         // オフセット (bias correction)
    float4 scale;          // スケール (precision adjustment)
    int interpolationMode; // 0=Nearest, 1=Linear, 2=Cubic, 3=Lanczos
    int3 _padding;         // アライメント用パディング
};

// テクスチャとサンプラー
Texture2D<int> texY  : register(t0);  // Y プレーン (輝度)
Texture2D<int> texCb : register(t1);  // Cb プレーン (青色差)
Texture2D<int> texCr : register(t2);  // Cr プレーン (赤色差)
SamplerState samplerLinear : register(s0);

static const float PI = 3.14159265359f;

// 頂点シェーダ入力
struct VSInput
{
    float3 position : POSITION;
    float2 texCoord : TEXCOORD0;
};

// ピクセルシェーダ入力
struct PSInput
{
    float4 position : SV_POSITION;
    float2 texCoord : TEXCOORD0;
};

// 頂点シェーダ
PSInput VSMain(VSInput input)
{
    PSInput output;
    output.position = float4(input.position, 1.0f);
    output.texCoord = input.texCoord;
    return output;
}

// Cubic (Catmull-Rom) weight function
float cubic_weight(float x)
{
    x = abs(x);
    float x2 = x * x;
    float x3 = x2 * x;
    if (x <= 1.0f)
    {
        return 1.5f * x3 - 2.5f * x2 + 1.0f;
    }
    else if (x <= 2.0f)
    {
        return -0.5f * x3 + 2.5f * x2 - 4.0f * x + 2.0f;
    }
    return 0.0f;
}

// Lanczos weight function (a=3)
float lanczos_weight(float x)
{
    if (x == 0.0f) return 1.0f;
    x = abs(x);
    if (x < 3.0f)
    {
        float pix = PI * x;
        return sin(pix) * sin(pix / 3.0f) / (pix * pix / 3.0f);
    }
    return 0.0f;
}

// YCbCr から RGB への変換ヘルパー
float4 ycbcr_to_rgba(float y, float cb, float cr)
{
    float4 ycbcr = float4(y, cb, cr, 1.0f);
    ycbcr = ycbcr * scale + offset;
    float4 rgba = mul(ycbcr, colorMatrix);
    rgba.a = 1.0f;
    return saturate(rgba);
}

// 単一ピクセルサンプリング (Nearest / Linear 用)
float3 sampleYCbCr(int2 pos, uint2 y_dim, uint2 c_dim)
{
    int2 y_pos = clamp(pos, int2(0, 0), int2(y_dim.x - 1, y_dim.y - 1));
    float2 uv = float2(pos) / float2(y_dim);
    int2 c_pos = int2(uv * float2(c_dim));
    c_pos = clamp(c_pos, int2(0, 0), int2(c_dim.x - 1, c_dim.y - 1));
    
    float y = (float)texY.Load(int3(y_pos, 0));
    float cb = (float)texCb.Load(int3(c_pos, 0));
    float cr = (float)texCr.Load(int3(c_pos, 0));
    return float3(y, cb, cr);
}

// ピクセルシェーダ (汎用 - 定数バッファから行列を使用)
float4 PSMain_Generic(PSInput input) : SV_TARGET
{
    uint y_width, y_height;
    texY.GetDimensions(y_width, y_height);
    uint2 y_dim = uint2(y_width, y_height);
    
    uint c_width, c_height;
    texCb.GetDimensions(c_width, c_height);
    uint2 c_dim = uint2(c_width, c_height);
    
    float2 texCoord = input.texCoord;
    
    // Nearest Neighbor または Linear (ハードウェアサンプラーを使えないのでどちらも点サンプリング)
    if (interpolationMode <= 1)
    {
        int2 i_pos = int2(texCoord.x * (float)y_width, texCoord.y * (float)y_height);
        i_pos = clamp(i_pos, int2(0, 0), int2(y_width - 1, y_height - 1));
        float y = (float)texY.Load(int3(i_pos, 0));
        
        int2 c_i_pos = int2(texCoord.x * (float)c_width, texCoord.y * (float)c_height);
        c_i_pos = clamp(c_i_pos, int2(0, 0), int2(c_width - 1, c_height - 1));
        
        float cb = (float)texCb.Load(int3(c_i_pos, 0));
        float cr = (float)texCr.Load(int3(c_i_pos, 0));
        
        return ycbcr_to_rgba(y, cb, cr);
    }
    
    // Cubic (4x4 サンプリング)
    if (interpolationMode == 2)
    {
        float2 pixelPos = texCoord * float2(y_dim) - 0.5f;
        float2 fracPart = frac(pixelPos);
        int2 basePos = int2(floor(pixelPos));
        
        float4 color = float4(0.0f, 0.0f, 0.0f, 0.0f);
        float totalWeight = 0.0f;
        
        [unroll]
        for (int j = -1; j <= 2; j++)
        {
            [unroll]
            for (int i = -1; i <= 2; i++)
            {
                int2 samplePos = basePos + int2(i, j);
                float3 ycbcr = sampleYCbCr(samplePos, y_dim, c_dim);
                
                float wx = cubic_weight((float)i - fracPart.x);
                float wy = cubic_weight((float)j - fracPart.y);
                float w = wx * wy;
                
                color += ycbcr_to_rgba(ycbcr.x, ycbcr.y, ycbcr.z) * w;
                totalWeight += w;
            }
        }
        return color / max(totalWeight, 0.001f);
    }
    
    // Lanczos3 (6x6 サンプリング)
    if (interpolationMode == 3)
    {
        float2 pixelPos = texCoord * float2(y_dim) - 0.5f;
        float2 fracPart = frac(pixelPos);
        int2 basePos = int2(floor(pixelPos));
        
        float4 color = float4(0.0f, 0.0f, 0.0f, 0.0f);
        float totalWeight = 0.0f;
        
        [unroll]
        for (int j = -2; j <= 3; j++)
        {
            [unroll]
            for (int i = -2; i <= 3; i++)
            {
                int2 samplePos = basePos + int2(i, j);
                float3 ycbcr = sampleYCbCr(samplePos, y_dim, c_dim);
                
                float wx = lanczos_weight((float)i - fracPart.x);
                float wy = lanczos_weight((float)j - fracPart.y);
                float w = wx * wy;
                
                color += ycbcr_to_rgba(ycbcr.x, ycbcr.y, ycbcr.z) * w;
                totalWeight += w;
            }
        }
        return color / max(totalWeight, 0.001f);
    }
    
    // Default fallback
    int2 i_pos = int2(texCoord.x * (float)y_width, texCoord.y * (float)y_height);
    i_pos = clamp(i_pos, int2(0, 0), int2(y_width - 1, y_height - 1));
    float y = (float)texY.Load(int3(i_pos, 0));
    
    int2 c_i_pos = int2(texCoord.x * (float)c_width, texCoord.y * (float)c_height);
    c_i_pos = clamp(c_i_pos, int2(0, 0), int2(c_width - 1, c_height - 1));
    
    float cb = (float)texCb.Load(int3(c_i_pos, 0));
    float cr = (float)texCr.Load(int3(c_i_pos, 0));
    
    return ycbcr_to_rgba(y, cb, cr);
}
