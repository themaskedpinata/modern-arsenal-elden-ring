// Run 8: AC-130 round tracers (mw2er_core::ac130_tracer). Screen-space capsules (a streak from the tail p0 to the head
// p1) and round impact flashes, one instance each, 6 vertices per instance, no vertex buffer. Drawn ADDITIVELY into
// the back buffer BEFORE the thermal pass, so the thermal film sees them as part of the scene (white hot in WHOT,
// black in BHOT). The CPU reference of the pixel shader is mw2er_core::ac130_tracer::shade.
//
// Root parameters: b0 = 4 constants (target width, height, 0, 0); t0 = root SRV of the instance ring slot.

cbuffer Consts : register(b0)
{
    float4 c_size;
};

struct Inst
{
    float2 p0;
    float2 p1;
    float half_w;
    float intensity;
    float tail;
    float kind;
};

StructuredBuffer<Inst> g_inst : register(t0);

struct VsOut
{
    float4 pos : SV_Position;
    // u along the segment from p0 (px), v across (px), segment length, half width
    nointerpolation float4 geo : TEXCOORD0;
    // u, v (interpolated)
    float2 uv : TEXCOORD1;
    // intensity, tail
    nointerpolation float2 look : TEXCOORD2;
};

VsOut vs_tracer(uint vid : SV_VertexID, uint iid : SV_InstanceID)
{
    Inst t = g_inst[iid];
    float2 d = t.p1 - t.p0;
    float len = length(d);
    float2 ax = len > 1e-3 ? d / len : float2(1.0, 0.0);
    float2 nx = float2(-ax.y, ax.x);
    float hw = max(t.half_w, 1e-3);
    float r = 3.5 * hw;   // mw2er_core::ac130_tracer::quad_margin
    // two triangles: (0,0) (1,0) (0,1) / (1,0) (1,1) (0,1)
    uint k = vid % 6;
    float cx = (k == 1 || k == 3 || k == 4) ? 1.0 : 0.0;
    float cy = (k == 2 || k == 4 || k == 5) ? 1.0 : 0.0;
    float u = lerp(-r, len + r, cx);
    float v = lerp(-r, r, cy);
    float2 px = t.p0 + ax * u + nx * v;
    VsOut o;
    o.pos = float4(px.x / c_size.x * 2.0 - 1.0, 1.0 - px.y / c_size.y * 2.0, 0.0, 1.0);
    o.geo = float4(0.0, 0.0, len, hw);
    o.uv = float2(u, v);
    o.look = float2(t.intensity, t.tail);
    return o;
}

float4 ps_tracer(VsOut i) : SV_Target
{
    float len = i.geo.z;
    float hw = i.geo.w;
    float s = clamp(i.uv.x, 0.0, len);
    float2 q = float2(i.uv.x - s, i.uv.y);
    float dist = length(q);
    float core = saturate(1.0 - dist / hw);
    float glow = exp(-(dist * dist) / (2.0 * hw * hw));
    float along = len > 1e-3 ? lerp(i.look.y, 1.0, s / len) : 1.0;
    float a = (core * core + 0.45 * glow) * along * i.look.x;
    // a warm white (only the luma matters under the thermal film; with thermal off it reads as a hot round)
    return float4(a, a * 0.97, a * 0.9, 0.0);
}
