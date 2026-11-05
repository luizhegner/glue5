// Display-only shaders for the GM45 D3D11 preview window.
// R32_FLOAT SRVs are also read through the red component of float4.

struct DisplayOut
{
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

DisplayOut VS_Display(uint vertexId : SV_VertexID)
{
    DisplayOut output;
    float2 p;
    if (vertexId == 0)
        p = float2(-1.0, -1.0);
    else if (vertexId == 1)
        p = float2(3.0, -1.0);
    else
        p = float2(-1.0, 3.0);

    output.position = float4(p, 0.0, 1.0);
    output.uv = float2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return output;
}

SamplerState PointClamp : register(s0);
Texture2D<float4> DisplayTexture : register(t0);

cbuffer DisplayCB : register(b0)
{
    float4 DisplayParams;
};

float4 PS_DisplayColor(DisplayOut input) : SV_Target0
{
    return float4(DisplayTexture.SampleLevel(PointClamp, input.uv, 0.0).rgb, 1.0);
}

float4 PS_DisplayScalar(DisplayOut input) : SV_Target0
{
    float value = DisplayTexture.SampleLevel(PointClamp, input.uv, 0.0).r * DisplayParams.x;
    value = saturate(value);
    return float4(value, value, value, 1.0);
}

float4 PS_DisplayOverlay(DisplayOut input) : SV_Target0
{
    return DisplayTexture.SampleLevel(PointClamp, input.uv, 0.0);
}
