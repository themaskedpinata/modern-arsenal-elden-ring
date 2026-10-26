// C6 depth fallback (work/c6/depth3.md 3; docs/C6_HOLD_DESIGN.md 13.2): depth-only capsules on the characters' body
// bones, drawn into OUR world-gun depth (reversed-Z, cleared to 0) right before the world guns, so a gun behind a body
// fails its GREATER depth test. No pixel shader, no render target: depth only. ER's depth is never bound or read.
//
// One instance per capsule; the mesh is generated from SV_VertexID: RINGS rows of SEGS vertices, rows 0..HALF on the
// hemisphere around `a` (from its pole to the equator), rows HALF+1..2*HALF+1 on the one around `b` (equator to pole);
// the band between rows HALF and HALF+1 is the cylinder. 6 vertices per quad, (2*HALF+1) x SEGS quads.

cbuffer Capsules : register(b0)
{
    row_major float4x4 view_proj;
    float4 caps[128];  // capsule k: caps[2k] = (a.xyz, r), caps[2k+1] = (b.xyz, 0)
};

static const uint SEGS = 16;
static const uint HALF = 4;

float4 vs_capsule(uint vid : SV_VertexID, uint iid : SV_InstanceID) : SV_Position
{
    float4 c0 = caps[iid * 2];
    float4 c1 = caps[iid * 2 + 1];
    float3 a = c0.xyz;
    float3 b = c1.xyz;
    float r = c0.w;
    float3 d = b - a;
    float len = length(d);
    float3 u = len > 1e-4 ? d / len : float3(0, 1, 0);
    float3 refv = abs(u.y) < 0.9 ? float3(0, 1, 0) : float3(1, 0, 0);
    float3 e1 = normalize(cross(u, refv));
    float3 e2 = cross(u, e1);

    uint quad = vid / 6;
    uint corner = vid % 6;
    // (dseg, drow) of the two triangles of a quad
    static const uint2 CORNERS[6] = { uint2(0, 0), uint2(1, 0), uint2(0, 1), uint2(0, 1), uint2(1, 0), uint2(1, 1) };
    uint seg = quad % SEGS + CORNERS[corner].x;
    uint row = quad / SEGS + CORNERS[corner].y;

    float t = 6.28318530718 * (float)(seg % SEGS) / (float)SEGS;
    float phi;
    float3 center;
    if (row <= HALF)
    {
        phi = -1.57079632679 + 1.57079632679 * (float)row / (float)HALF;
        center = a;
    }
    else
    {
        phi = 1.57079632679 * (float)(row - HALF - 1) / (float)HALF;
        center = b;
    }
    float3 p = center + r * (cos(phi) * (cos(t) * e1 + sin(t) * e2) + sin(phi) * u);
    return mul(view_proj, float4(p, 1.0));
}
