// C6 depth 5 (docs/C6_HOLD_DESIGN.md part 15): ER's scene depth, read only, through OUR SRV (plane 0,
// R32_FLOAT_X8X24_TYPELESS) while ER left it in a readable state (proven per frame by the barrier fold); ER's resource
// is never written, barriered or bound for writing.
//
// - cs_copy: the FARTHEST (smallest reverse-Z value) of each 2x2 block into OUR half-resolution R32_FLOAT texture
//   (occluder edges shrink by at most one pixel, never grow; ER's TAA jitter of <= 0.5 px is absorbed).
// - cs_sample (probe P-S2): 22 texels (16 grid points, the centre, our chest pixel and its +-8 px cross: C6 round 7)
//   into OUR structured buffer.
// - vs_full / ps_prime: a full-screen depth-only pass into OUR world-gun depth (depth func ALWAYS on its cleared 0):
//   ER's depth linearised with ER's near / far, pushed back by bias_m + bias_rel * z, written in our reversed-Z
//   projection (ndc = our_a + our_b / z). The sky (0) is discarded. No render target.

cbuffer Reader : register(b0)
{
    uint4 dims;    // x = width, y = height of ER's depth (full resolution)
    uint4 pts[11]; // SAMPLE_N = 22 sample points (x, y): point 2k in .xy, point 2k+1 in .zw
};

Texture2D<float> er_depth : register(t0);
RWTexture2D<float> copy_out : register(u0);
RWStructuredBuffer<float> sample_out : register(u1);

[numthreads(8, 8, 1)]
void cs_copy(uint3 id : SV_DispatchThreadID)
{
    uint w = dims.x;
    uint h = dims.y;
    uint2 o = id.xy;
    if (o.x >= (w + 1) / 2 || o.y >= (h + 1) / 2)
    {
        return;
    }
    uint2 s0 = o * 2;
    uint2 s1 = min(s0 + 1, uint2(w - 1, h - 1));
    float a = er_depth.Load(int3(s0, 0));
    float b = er_depth.Load(int3(s1.x, s0.y, 0));
    float c = er_depth.Load(int3(s0.x, s1.y, 0));
    float d = er_depth.Load(int3(s1, 0));
    copy_out[o] = min(min(a, b), min(c, d));
}

[numthreads(32, 1, 1)]
void cs_sample(uint3 id : SV_DispatchThreadID)
{
    uint i = id.x;
    if (i >= 22)
    {
        return;
    }
    uint4 p = pts[i >> 1];
    uint2 xy = (i & 1) ? p.zw : p.xy;
    sample_out[i] = er_depth.Load(int3(xy, 0));
}

cbuffer Prime : register(b0)
{
    float near_er;
    float far_er;     // <= near_er: an infinite far plane
    float bias_m;
    float bias_rel;
    float our_a;
    float our_b;
    uint half_res;
    uint pad0;
};

Texture2D<float> er_copy : register(t0);

float4 vs_full(uint vid : SV_VertexID) : SV_Position
{
    float2 uv = float2((vid << 1) & 2, vid & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float ps_prime(float4 pos : SV_Position) : SV_Depth
{
    uint2 p = uint2(pos.xy);
    if (half_res != 0)
    {
        p = p >> 1;
    }
    float d = er_copy.Load(int3(p, 0));
    // NaN / inf (exponent all ones, tested on the bits: fxc assumes no NaN in float compares) and the sky (0)
    if ((asuint(d) & 0x7f800000) == 0x7f800000 || d <= 0.0)
    {
        discard;
    }
    float z = far_er > near_er ? near_er * far_er / (d * (far_er - near_er) + near_er) : near_er / d;
    z = z + bias_m + bias_rel * z;
    return saturate(our_a + our_b / z);
}
