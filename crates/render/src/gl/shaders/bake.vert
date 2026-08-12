#version 300 es

out vec2 v_uv;

// One oversized triangle covering the destination. No vertex buffer, no attributes.
// Unlike the composite pass this samples the whole source and does not flip: both
// textures are stored with row 0 at the top, so the copy is orientation-preserving.
void main() {
    vec2 at = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(at * 2.0 - 1.0, 0.0, 1.0);
    v_uv = at;
}
