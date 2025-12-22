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
Texture2D<int> texY  : register(t0);  // Y プレーン (輝度)
Texture2D<int> texCb : register(t1);  // Cb プレーン (青色差)
Texture2D<int> texCr : register(t2);  // Cr プレーン (赤色差)
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

// ピクセルシェーダ (汎用 - 定数バッファから行列を使用)
float4 PSMain_Generic(PSInput input) : SV_TARGET
{
    // 整数テクスチャ (SINT) なので Load() を使用
    uint width, height;
    texY.GetDimensions(width, height);
    
    int2 i_pos = int2(input.texCoord.x * (float)width, input.texCoord.y * (float)height);
    i_pos = clamp(i_pos, int2(0, 0), int2(width - 1, height - 1));
    float y  = (float)texY.Load(int3(i_pos, 0));
    
    // Cb/Cr はサブサンプリングされている可能性があるため個別に取得
    uint c_width, c_height;
    texCb.GetDimensions(c_width, c_height);
    int2 c_i_pos = int2(input.texCoord.x * (float)c_width, input.texCoord.y * (float)c_height);
    c_i_pos = clamp(c_i_pos, int2(0, 0), int2(c_width - 1, height - 1));
    
    float cb = (float)texCb.Load(int3(c_i_pos, 0));
    float cr = (float)texCr.Load(int3(c_i_pos, 0));
    
    // オフセットとスケール補正
    float4 ycbcr = float4(y, cb, cr, 1.0f);
    ycbcr = ycbcr * scale + offset;
    
    // 行列変換 (mul(vector, matrix) は定数バッファの各行を各出力チャンネルにマッピング)
    float4 rgba = mul(ycbcr, colorMatrix);
    rgba.a = 1.0f;
    
    return saturate(rgba);
}
