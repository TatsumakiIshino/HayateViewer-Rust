// YCbCr to RGB conversion shader for Direct3D 11
// Supports BT.601 and BT.709 color spaces
// Input: Y, Cb, Cr planes as separate textures (R16_SNORM)
// Output: RGBA color

// 定数バッファ：変換行列とオフセット
cbuffer YCbCrParams : register(b0)
{
    float4x4 colorMatrix;  // YCbCr to RGB 変換行列
    float4 offset;         // オフセット (bias correction)
    float4 scale;          // スケール (precision adjustment)
};

// テクスチャとサンプラー
Texture2D<float> texY  : register(t0);  // Y プレーン (輝度)
Texture2D<float> texCb : register(t1);  // Cb プレーン (青色差)
Texture2D<float> texCr : register(t2);  // Cr プレーン (赤色差)
SamplerState samplerLinear : register(s0);

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

// ピクセルシェーダ (BT.601 フルレンジ)
// Y:  [0, 1] -> [0, 255]
// Cb: [-0.5, 0.5] -> [-128, 127]
// Cr: [-0.5, 0.5] -> [-128, 127]
float4 PSMain_BT601(PSInput input) : SV_TARGET
{
    float y  = texY.Sample(samplerLinear, input.texCoord);
    float cb = texCb.Sample(samplerLinear, input.texCoord);
    float cr = texCr.Sample(samplerLinear, input.texCoord);
    
    // 符号付きサンプルをオフセット補正 (hayro-jpeg2000 は符号付き)
    // SNORM 形式: [-1, 1] にマッピングされている
    // 実際の値域に変換
    y  = y * scale.x + offset.x;
    cb = cb * scale.y + offset.y;
    cr = cr * scale.z + offset.z;
    
    // BT.601 変換行列
    // R = Y + 1.402 * Cr
    // G = Y - 0.344136 * Cb - 0.714136 * Cr
    // B = Y + 1.772 * Cb
    float r = y + 1.402f * cr;
    float g = y - 0.344136f * cb - 0.714136f * cr;
    float b = y + 1.772f * cb;
    
    return float4(saturate(r), saturate(g), saturate(b), 1.0f);
}

// ピクセルシェーダ (BT.709)
// HDTV 向けの色空間
float4 PSMain_BT709(PSInput input) : SV_TARGET
{
    float y  = texY.Sample(samplerLinear, input.texCoord);
    float cb = texCb.Sample(samplerLinear, input.texCoord);
    float cr = texCr.Sample(samplerLinear, input.texCoord);
    
    y  = y * scale.x + offset.x;
    cb = cb * scale.y + offset.y;
    cr = cr * scale.z + offset.z;
    
    // BT.709 変換行列
    // R = Y + 1.5748 * Cr
    // G = Y - 0.1873 * Cb - 0.4681 * Cr
    // B = Y + 1.8556 * Cb
    float r = y + 1.5748f * cr;
    float g = y - 0.1873f * cb - 0.4681f * cr;
    float b = y + 1.8556f * cb;
    
    return float4(saturate(r), saturate(g), saturate(b), 1.0f);
}

// ピクセルシェーダ (汎用 - 定数バッファから行列を使用)
float4 PSMain_Generic(PSInput input) : SV_TARGET
{
    float y  = texY.Sample(samplerLinear, input.texCoord);
    float cb = texCb.Sample(samplerLinear, input.texCoord);
    float cr = texCr.Sample(samplerLinear, input.texCoord);
    
    // オフセットとスケール補正
    float4 ycbcr = float4(y, cb, cr, 1.0f);
    ycbcr = ycbcr * scale + offset;
    
    // 行列変換
    float4 rgba = mul(colorMatrix, ycbcr);
    rgba.a = 1.0f;
    
    return saturate(rgba);
}
