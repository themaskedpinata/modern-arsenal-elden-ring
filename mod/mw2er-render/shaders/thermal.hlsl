// M4 thermal pass (docs/M4_DESIGN.md section 6.4): MW2's AC-130 film tweak over Elden Ring's finished frame, the
// per-gun overlay in the centred 4:3 box and the grain. One full-screen triangle; the scene copy is read with Load
// (1:1). Values stay in stored (sRGB-encoded) space like MW2's 2D path. The CPU reference is
// mw2er_core::thermal::{shade_pixel, compose}; vmtest compares the two.
//
// Root constants (b0, 32 DWORDs):
//   c0 = base.rgb, offset      c1 = delta.rgb, desat     c2 = quad.rgb, fade     c3 = luma.rgb, overlay alpha
//   c4 = box x0, y0, w, h (px) c5 = grain uv offset.xy, scale.xy               c6 = grain tint.rgb, grain alpha
//   c7 = x: overlay on, y: grain on, z: target width, w: target height
//   c8 = x: mid-tone gamma of the film input (run 8; 1 = MW2 as is; CPU reference mw2er_core::thermal::lift)

cbuffer Consts : register(b0)
{
    float4 c_base_offset;
    float4 c_delta_desat;
    float4 c_quad_fade;
    float4 c_luma_ovalpha;
    float4 c_box;
    float4 c_grain_uv;
    float4 c_grain_tint;
    float4 c_flags;
    float4 c_extra;
};

Texture2D<float4> t_scene : register(t0);
Texture2D<float4> t_overlay : register(t1);
Texture2D<float4> t_grain : register(t2);
SamplerState s_clamp : register(s0);
SamplerState s_wrap : register(s1);

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

float4 ps_thermal(VsOut i) : SV_Target
{
    int2 p = int2(i.pos.xy);
    float3 c = t_scene.Load(int3(p, 0)).rgb;
    float g = dot(c, c_luma_ovalpha.rgb);
    if (c_extra.x != 1.0)
    {
        g = pow(max(g, 0.0), c_extra.x);
    }
    float3 cd = lerp(c, float3(g, g, g), c_delta_desat.w);
    float3 tint = c_base_offset.rgb + c_delta_desat.rgb * g + c_quad_fade.rgb * (g * g);
    float3 x = saturate(cd * tint + c_base_offset.w) * c_quad_fade.w;
    if (c_flags.x > 0.5)
    {
        float2 uvb = (i.pos.xy - c_box.xy) / c_box.zw;
        if (uvb.x >= 0.0 && uvb.y >= 0.0 && uvb.x < 1.0 && uvb.y < 1.0)
        {
            float4 ov = t_overlay.SampleLevel(s_clamp, uvb, 0.0);
            x = lerp(x, ov.rgb, ov.a * c_luma_ovalpha.w);
        }
    }
    if (c_flags.y > 0.5)
    {
        float2 uvs = i.pos.xy / c_flags.zw * c_grain_uv.zw + c_grain_uv.xy;
        float4 gr = t_grain.SampleLevel(s_wrap, uvs, 0.0);
        x = lerp(x, c_grain_tint.rgb * gr.rgb, gr.a * c_grain_tint.w);
    }
    return float4(x, 1.0);
}
