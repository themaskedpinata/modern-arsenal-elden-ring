// C4.1 probe P2 (docs/C4_1_DESIGN.md 2.1 / 2.4): reads 16 texels of ER's UI layer (an SRV of OUR heap on ER's
// resource, which is in PIXEL | NON_PIXEL_SHADER_RESOURCE when this runs) and writes them packed to OUR UAV buffer.
// No sampler, no filtering: Load at integer texel coordinates. The layer is never written.

cbuffer SamplePoints : register(b0)
{
    uint4 pts[8]; // 16 points (x, y): point 2k in .xy, point 2k+1 in .zw
};

Texture2D<float4> layer : register(t0);
RWStructuredBuffer<uint> texels : register(u0);

[numthreads(16, 1, 1)]
void cs_sample(uint3 id : SV_DispatchThreadID)
{
    uint i = id.x;
    uint4 p = pts[i >> 1];
    uint2 xy = (i & 1) ? p.zw : p.xy;
    uint4 c = (uint4)round(saturate(layer.Load(int3(xy, 0))) * 255.0);
    texels[i] = c.r | (c.g << 8) | (c.b << 16) | (c.a << 24);
}
