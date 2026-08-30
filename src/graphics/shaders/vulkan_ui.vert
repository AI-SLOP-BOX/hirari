#version 450

layout(location = 0) out vec4 vColor;

struct Rect {
    vec4 pos;     // x, y, w, h in normalized device coordinates
    vec4 props;   // radius, border, shadow, isTex
    vec4 color;   // RGBA
};

layout(set = 0, binding = 0) uniform Uniforms {
    Rect rects[1024];
} ubo;

void main() {
    // A unit quad generated from the vertex index keeps the first pipeline
    // completely vertex-buffer free. Later retained-mode batches can replace
    // this with an instanced vertex input without changing the fragment ABI.
    const vec2 quad[4] = vec2[](
        vec2(0.0, 0.0), vec2(1.0, 0.0),
        vec2(0.0, 1.0), vec2(1.0, 1.0)
    );
    Rect rect = ubo.rects[gl_InstanceIndex];
    vec2 position = rect.pos.xy + quad[gl_VertexIndex] * rect.pos.zw;
    gl_Position = vec4(position, 0.0, 1.0);
    vColor = rect.color;
}
