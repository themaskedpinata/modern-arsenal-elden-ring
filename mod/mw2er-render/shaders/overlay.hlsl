// M6: the sniper scope overlay (docs/M6_DESIGN.md section 6.5), a full-screen pass after the viewmodel.
// The overlay texture is a black mask (lens see-through, reticle and surround opaque) drawn as a square of the screen
// height, centred; the side bars outside the square are black. Output is black with the mask's alpha (srcalpha /
// invsrcalpha), so it is the same in every output mode (sRGB, PQ, scRGB: black is 0 in all of them).

Texture2D overlay_tex : register(t0);
SamplerState overlay_samp : register(s0);

cbuffer OverlayConsts : register(b0)
{
    float2 target_px;   // width, height of the back buffer
    float opacity;      // 0..1 (the ADS fade-in)
    float pad;
};

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut vs_overlay(uint id : SV_VertexID)
{
    // one triangle covering the screen: uv (0,0) top-left .. (1,1) bottom-right
    float2 p = float2((id << 1) & 2, id & 2);
    VsOut o;
    o.pos = float4(p * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = p;
    return o;
}

float4 ps_overlay(VsOut i) : SV_Target
{
    float w = target_px.x;
    float h = target_px.y;
    float u = (i.uv.x * w - (w - h) * 0.5) / max(h, 1.0);
    float a = 1.0;
    if (u >= 0.0 && u <= 1.0)
    {
        a = overlay_tex.SampleLevel(overlay_samp, float2(u, i.uv.y), 0).a;
    }
    return float4(0.0, 0.0, 0.0, saturate(a * opacity));
}
