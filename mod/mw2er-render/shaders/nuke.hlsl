// M7 nuke post pass (docs/M7_DESIGN.md 3.7): MW2's mpnuke / mpnuke_aftermath visions (glow + film tweak) plus a white
// flash over Elden Ring's finished frame. Two full-screen triangles:
//   ps_bloom: the scene copy box-filtered to 1/8 resolution (16 bilinear taps = the 8x8 block), bright-passed
//             (k = max(l - cutoff, 0) / l, Rec. 709 luma) and desaturated by r_glowBloomDesaturation;
//   ps_nuke:  scene + 9-tap tent of the bloom x r_glowBloomIntensity0 -> saturate -> film (the AC-130 film model:
//             desat, base + delta g + quad g^2, offset) -> lerp(scene, film, amount) -> lerp(out, 1, flash).
// The CPU reference is mw2er_core::nuke_fx::{bright, composite}; vmtest compares the two.
//
// Root constants (b0, 24 DWORDs; nuke_fx::constants):
//   c0 = film base.rgb, offset   c1 = delta.rgb, desat   c2 = quad.rgb, amount
//   c3 = cutoff, glow desat, intensity, flash            c4 = 1 / bloom w, 1 / bloom h, tap spacing, 0
//   c5 = target w, h, bloom w, h

cbuffer Consts : register(b0)
{
    float4 c_base_offset;
    float4 c_delta_desat;
    float4 c_quad_amount;
    float4 c_glow;
    float4 c_bloom_texel;
    float4 c_sizes;
};

Texture2D<float4> t_scene : register(t0);
Texture2D<float4> t_bloom : register(t1);
SamplerState s_clamp : register(s0);

static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

struct VsOut
{
    float4 pos : SV_Position;
};

VsOut vs_fullscreen(uint id : SV_VertexID)
{
    float2 uv = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

float4 ps_bloom(VsOut i) : SV_Target
{
    float2 scene_texel = 1.0 / c_sizes.xy;
    // the 8x8 block of this bloom texel: 4x4 bilinear taps, each at the corner of 4 scene texels
    float2 base = (floor(i.pos.xy) * 8.0) * scene_texel;
    float3 acc = float3(0.0, 0.0, 0.0);
    [unroll] for (int y = 0; y < 4; y++)
    {
        [unroll] for (int x = 0; x < 4; x++)
        {
            float2 uv = base + float2(2.0 * x + 1.0, 2.0 * y + 1.0) * scene_texel;
            acc += t_scene.SampleLevel(s_clamp, uv, 0.0).rgb;
        }
    }
    float3 a = acc / 16.0;
    float l = dot(a, LUMA);
    float k = max(l - c_glow.x, 0.0) / max(l, 1e-4);
    float3 b = a * k;
    float lb = dot(b, LUMA);
    b = b + (float3(lb, lb, lb) - b) * c_glow.y;
    return float4(b, 1.0);
}

float4 ps_nuke(VsOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    float3 scene = t_scene.Load(int3(p, 0)).rgb;
    // the bloom: a 9-tap tent around this pixel's bloom position (spacing in bloom texels)
    float2 uv = i.pos.xy / c_sizes.xy;
    float2 d = c_bloom_texel.xy * c_bloom_texel.z;
    float3 bloom = float3(0.0, 0.0, 0.0);
    [unroll] for (int y = -1; y <= 1; y++)
    {
        [unroll] for (int x = -1; x <= 1; x++)
        {
            float w = (2.0 - abs((float)x)) * (2.0 - abs((float)y)) / 16.0;
            bloom += t_bloom.SampleLevel(s_clamp, uv + float2(x, y) * d, 0.0).rgb * w;
        }
    }
    float3 c = saturate(scene + bloom * c_glow.z);
    float g = dot(c, LUMA);
    float3 cd = lerp(c, float3(g, g, g), c_delta_desat.w);
    float3 tint = c_base_offset.rgb + c_delta_desat.rgb * g + c_quad_amount.rgb * (g * g);
    float3 film = saturate(cd * tint + c_base_offset.w);
    float3 o = scene + (film - scene) * saturate(c_quad_amount.w);
    o = o + (float3(1.0, 1.0, 1.0) - o) * saturate(c_glow.w);
    return float4(o, 1.0);
}
