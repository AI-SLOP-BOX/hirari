#include <metal_stdlib>
using namespace metal;

/**
 * @brief Aura Cinematic Waveform: Light-Textured Density Rendering.
 * Replaces basic polylines with glowing fragments that represent energy 
 * through additive blending and sub-pixel sample markers.
 */

struct VertexOut {
    uint vertexID [[vertex_id]];
    float4 position [[position]];
    float energy; // Peak strength for glow calculation
    float4 baseColor;
};

struct WaveformData {
    float2 minMax; // [min_peak, max_peak] at this window
};

vertex VertexOut waveform_vertex(
    uint vid [[vertex_id]],
    device WaveformData *data [[buffer(0)]],
    constant float2 &view_range [[buffer(1)]],
    constant float4 &draw_color [[buffer(2)]],
    constant uint &num_samples [[buffer(3)]]
) {
    VertexOut out;
    
    // --- INDUSTRIAL SAFE SCALING ---
    uint sampleIdx = vid / 2;
    float rangeWidth = std::max(1.0f, view_range.y - view_range.x);
    float x = (float)sampleIdx / rangeWidth;
    float y = (vid % 2 == 0) ? data[sampleIdx].minMax.x : data[sampleIdx].minMax.y;
    
    // Normalize to Viewport space (-1, 1)
    out.position = float4(x * 2.0 - 1.0, y, 0.0, 1.0);
    out.energy = std::clamp(std::abs(y), 0.05f, 1.0f);
    out.baseColor = draw_color;
    
    return out;
}

fragment float4 waveform_fragment(
    VertexOut in [[stage_in]],
    float2 pointCoord [[point_coord]]
) {
    // --- INDUSTRIAL CINEMATIC: LIQUID DENSITY ---
    // Higher energy peaks have higher density (alpha) and subtle bloom.
    float4 base = in.baseColor;
    
    // 1. Density Alpha: Thickens the visualization at high energy
    float density = std::pow(in.energy, 0.65f);
    base.a *= density;
    
    // 2. Sub-pixel Glow: Additive 'Liquid Aura' bloom
    // We mix a 'Primary Electric' blue (Logic 11 style) into the highlight.
    float4 bloom = float4(0.3f, 0.7f, 1.0f, 0.0f) * std::pow(in.energy, 2.0f);
    
    // 3. Antialiasing Hack: Soften edges by modulating alpha near peaks
    float edgeSoftness = 1.0f - std::smoothstep(0.9f, 1.0f, std::abs(in.position.y));
    base.a *= edgeSoftness;
    
    return base + bloom;
}
