// MW2 viewmodel (gun + hands) for mw2er.dll and vmtest.exe (docs/M3_DESIGN.md section 4.3).
// Compiled offline by build.rs with fxc (vs_5_1 / ps_5_1); both binaries embed the same bytes.
//
// Space: vertices are in MW2 model space (inches). The bone palette (3x4, row-major) skins them; view_proj maps
// MW2 space to the viewmodel's own reversed-Z clip space; view (3x3 part) maps normals into D3D view space for
// the light. Lighting: one directional light + a sky/ground hemisphere ambient on the MW2 colour map (sampled
// through an _SRGB view, so linear). Output transform by out_mode: 0 sRGB OETF, 1 PQ (BT.2020), 2 scRGB.

cbuffer FrameConsts : register(b0)
{
    row_major float4x4 view_proj;
    row_major float4x4 view;
    float3 light_dir_view;   // direction the light travels, view space
    float  diffuse;
    float3 ambient_sky;
    float  exposure;
    float3 ambient_ground;
    uint   out_mode;
    float  paper_white_nits;
    uint   debug_mode;       // 0 normal, 1 flat part colour (gun red, hands green), 2 white silhouette
    uint   cut_count;        // C5 cutout: regions in cut_rect (0 = none: exactly the draw of before)
    uint   cut_pad;
    uint4  cut_rect[4];      // see cutout.hlsli
};

#include "cutout.hlsli"

cbuffer Bones : register(b1)
{
    row_major float3x4 bones[128];
};

cbuffer DrawConsts : register(b2)
{
    uint part_id;            // 0 gun, 1 hands
    uint alpha_mode;         // 0 opaque, 1 alpha-tested + blended
    uint2 draw_pad;
};

Texture2D    color_map : register(t0);
SamplerState samp      : register(s0);

struct VsIn
{
    float3 pos     : POSITION;
    float4 normal  : NORMAL;
    float2 uv      : TEXCOORD0;
    uint4  joints  : BLENDINDICES;
    float4 weights : BLENDWEIGHT;
    float4 color   : COLOR0;
};

struct VsOut
{
    float4 pos    : SV_Position;
    float3 normal : NORMAL;
    float2 uv     : TEXCOORD0;
};

VsOut vs_main(VsIn i)
{
    float3x4 skin = bones[i.joints.x] * i.weights.x
                  + bones[i.joints.y] * i.weights.y
                  + bones[i.joints.z] * i.weights.z
                  + bones[i.joints.w] * i.weights.w;
    float3 p = mul(skin, float4(i.pos, 1.0));
    float3 n = mul((float3x3)skin, i.normal.xyz);
    VsOut o;
    o.pos = mul(view_proj, float4(p, 1.0));
    o.normal = mul((float3x3)view, n);
    o.uv = i.uv;
    return o;
}

float3 srgb_oetf(float3 c)
{
    c = saturate(c);
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    return float3(c.r <= 0.0031308 ? lo.r : hi.r, c.g <= 0.0031308 ? lo.g : hi.g, c.b <= 0.0031308 ? lo.b : hi.b);
}

float3 pq_oetf(float3 c709)
{
    const float3x3 to2020 = float3x3(0.6274040, 0.3292820, 0.0433136,
                                     0.0690970, 0.9195400, 0.0113612,
                                     0.0163916, 0.0880132, 0.8955950);
    float3 y = saturate(mul(to2020, max(c709, 0.0)) * (paper_white_nits / 10000.0));
    const float m1 = 0.1593017578125, m2 = 78.84375;
    const float c1 = 0.8359375, c2 = 18.8515625, c3 = 18.6875;
    float3 ym = pow(y, m1);
    return pow((c1 + c2 * ym) / (1.0 + c3 * ym), m2);
}

float4 ps_main(VsOut i) : SV_Target
{
    float4 tex = color_map.Sample(samp, i.uv);
    float alpha = 1.0;
    if (alpha_mode == 1)
    {
        clip(tex.a - 1.0 / 255.0);
        alpha = tex.a;
    }
    // C5: ER's message regions are left alone (opaque: dithered discard in the feather, alpha draws: alpha x keep)
    if (cut_count != 0)
    {
        float keep = cut_keep(i.pos.xy);
        if (keep <= 0.0 || (alpha_mode == 0 && keep < cut_dither(i.pos.xy)))
            discard;
        alpha *= keep;
    }
    if (debug_mode == 1)
        return float4(part_id == 0 ? float3(1, 0, 0) : float3(0, 1, 0), 1.0);
    if (debug_mode == 2)
        return float4(1, 1, 1, 1);
    float3 n = normalize(i.normal);
    float ndl = max(dot(n, -light_dir_view), 0.0);
    float3 amb = lerp(ambient_ground, ambient_sky, n.y * 0.5 + 0.5);
    float3 c = tex.rgb * (ndl * diffuse + amb) * exposure;
    float3 o;
    if (out_mode == 1)
        o = pq_oetf(c);
    else if (out_mode == 2)
        o = c * (paper_white_nits / 80.0);
    else
        o = srgb_oetf(c);
    return float4(o, alpha);
}
