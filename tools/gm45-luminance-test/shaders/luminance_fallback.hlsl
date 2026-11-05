// GLUE5 GM45 fallback prototype
// Captured source families:
//   9b8e0c6c85e065da avgLuminanceCompute::LuminanceDownsampleInitCS
//   7a1e45a48ffaaf08 avgLuminanceCompute::LuminanceDownsampleCS
//
// These are pixel-shader replacements for two narrowly identified compute
// shaders. They are NOT a general compute-to-pixel-shader translator.
//
// Host requirements:
// - Compile VS_Fullscreen as vs_4_0.
// - Compile each PS entry point as ps_4_0.
// - Draw one full-screen triangle, with culling disabled.
// - Disable blending and depth/stencil for these passes.
// - Set viewport/scissor to the destination texture dimensions.
// - Bind the source at t3 for PS_LuminanceTile, or t2 for PS_LuminanceFinal.
// - Bind the captured b12 constant vector. Its z/w components are the source
//   width/height for the corresponding pass.
// - Bind the destination as an R32_FLOAT render target (RTV).
//
// Important semantic detail: the first pass writes each tile's contribution
// divided by the FULL original image area. The second pass sums those
// contributions. Do not change stage 1 to a per-tile average.

cbuffer CB12 : register(b12)
{
    // Captured WGSL reads cb12_12.tint_symbol[0].zw as source dimensions.
    float4 LuminanceParams;
};

static const float3 LUMINANCE_WEIGHTS =
    float3(0.21250000596046447754,
           0.71539998054504394531,
           0.0720999985933303833);

struct VSOutput
{
    float4 position : SV_Position;
};

VSOutput VS_Fullscreen(uint vertexId : SV_VertexID)
{
    VSOutput output;
    float2 p;
    if (vertexId == 0)
        p = float2(-1.0, -1.0);
    else if (vertexId == 1)
        p = float2(3.0, -1.0);
    else
        p = float2(-1.0, 3.0);

    output.position = float4(p, 0.0, 1.0);
    return output;
}

// Replaces avgLuminanceCompute::LuminanceDownsampleInitCS.
// Input:  t3, float-compatible view of the captured source texture.
// Output: R32_FLOAT RTV, dimensions ceil(sourceWidth/16) x ceil(sourceHeight/16).
//
// One output pixel corresponds to one original 16x16 compute workgroup.
// The sequential reduction intentionally avoids groupshared memory,
// workgroup barriers, atomics and subgroup operations.
Texture2D<float4> LuminanceSource : register(t3);

float PS_LuminanceTile(VSOutput input) : SV_Target0
{
    int2 sourceSize = int2(max(LuminanceParams.zw, float2(0.0, 0.0)));
    if (sourceSize.x <= 0 || sourceSize.y <= 0)
        return 0.0;

    int2 tile = int2(input.position.xy);
    int2 tileBase = tile * 16;

    float sum = 0.0;

    [loop]
    for (int y = 0; y < 16; ++y)
    {
        [loop]
        for (int x = 0; x < 16; ++x)
        {
            int2 pixel = tileBase + int2(x, y);
            if (pixel.x < sourceSize.x && pixel.y < sourceSize.y)
            {
                float3 rgb = LuminanceSource.Load(int3(pixel, 0)).rgb;
                float luminance = dot(rgb, LUMINANCE_WEIGHTS);

                // The captured compute shader replaces NaN and +/-Inf
                // luminance values with zero before reduction.
                if (!isfinite(luminance))
                    luminance = 0.0;

                sum += luminance;
            }
        }
    }

    // Preserve the original shader's normalization by the entire source area.
    float sourceArea = float(sourceSize.x) * float(sourceSize.y);
    return sum / sourceArea;
}

// Replaces avgLuminanceCompute::LuminanceDownsampleCS.
// Input:  t2, R32_FLOAT partial contributions from PS_LuminanceTile.
// Output: a 1x1 R32_FLOAT RTV.
//
// The captured compute shader dispatches exactly one 16x16 workgroup here.
// Therefore its input reduction grid must not exceed 16x16. The host must
// reject unsupported dimensions rather than silently accepting a truncated
// sum; this guard makes the shader's behavior explicit if the contract breaks.
Texture2D<float> LuminancePartials : register(t2);

float PS_LuminanceFinal(VSOutput input) : SV_Target0
{
    int2 partialSize = int2(max(LuminanceParams.zw, float2(0.0, 0.0)));
    if (partialSize.x <= 0 || partialSize.y <= 0 ||
        partialSize.x > 16 || partialSize.y > 16)
        return 0.0;

    float sum = 0.0;

    [loop]
    for (int y = 0; y < 16; ++y)
    {
        [loop]
        for (int x = 0; x < 16; ++x)
        {
            if (x < partialSize.x && y < partialSize.y)
                sum += LuminancePartials.Load(int3(x, y, 0));
        }
    }

    // Stage 1 already divided every contribution by original width*height.
    // Stage 2 only sums the contributions; it does not divide again.
    return sum;
}
