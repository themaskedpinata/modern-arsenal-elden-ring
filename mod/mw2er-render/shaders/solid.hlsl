// C4.1 PASS I41 (docs/C4_1_DESIGN.md 3.6): one constant colour over the viewport / scissor (the session canary: a 1x1
// texel at (2, 2) of ER's UI layer, written without blending or encoding). vmtest also draws its ER-like "UI" quads
// with it (blended "over" in the layer's convention).

cbuffer SolidConsts : register(b0)
{
    float4 color;
};

float4 vs_solid(uint id : SV_VertexID) : SV_Position
{
    // one triangle covering the viewport
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float4 ps_solid() : SV_Target
{
    return color;
}
