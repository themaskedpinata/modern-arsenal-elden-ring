// C5 `[ui_yield] mode = "cutout"`: screen regions (ER's message boxes) our draws leave alone, with a soft edge.
// Included by viewmodel.hlsl (the gun and the world guns). `cut_count` and `cut_rect` live in FrameConsts:
//   cut_rect[k] = (x0 | y0 << 16, x1 | y1 << 16, asuint(strength 0..1), asuint(feather px)), pixels of the target.
// Inside a region the draw keeps 1 - strength (0 = untouched); the feather band outside it ramps back to 1.

static const uint CUT_BAYER4[16] = { 0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5 };

// 0..1: how much of the draw is kept at pixel centre `px` (SV_Position.xy).
float cut_keep(float2 px)
{
    float cut = 0.0;
    [loop]
    for (uint k = 0; k < min(cut_count, 4u); k++)
    {
        uint4 r = cut_rect[k];
        float4 b = float4(r.x & 0xffff, r.x >> 16, r.y & 0xffff, r.y >> 16);
        float s = saturate(asfloat(r.z));
        float f = max(asfloat(r.w), 0.001);
        float2 d = min(px - b.xy, b.zw - px);       // >= 0.5 for pixel centres inside
        float e = saturate(1.0 + min(d.x, d.y) / f); // 1 inside, 0 at f px outside
        e = e * e * (3.0 - 2.0 * e);
        cut = max(cut, s * e);
    }
    return 1.0 - cut;
}

// Ordered-dither threshold for opaque draws (no blending in their PSO): 16 levels in (0, 1).
float cut_dither(float2 px)
{
    uint2 p = uint2(px) & 3;
    return (CUT_BAYER4[p.y * 4 + p.x] + 0.5) / 16.0;
}
