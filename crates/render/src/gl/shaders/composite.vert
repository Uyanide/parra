#version 300 es

// Region of each image to show, as (u0, v0, u1, v1) with v0 at the top row.
uniform vec4 u_uv;
uniform vec4 u_uv_p;

out vec2 v_uv;
out vec2 v_uv_p;

// One oversized triangle covering the viewport. No vertex buffer, no attributes.
vec2 corner(int index) {
    return vec2(float((index << 1) & 2), float(index & 2));
}

// Row 0 of the texture holds the top of the image, so the bottom of the screen
// samples v1 and the top samples v0.
vec2 region(vec4 uv, vec2 at) {
    return vec2(mix(uv.x, uv.z, at.x), mix(uv.w, uv.y, at.y));
}

void main() {
    vec2 at = corner(gl_VertexID);
    gl_Position = vec4(at * 2.0 - 1.0, 0.0, 1.0);
    v_uv = region(u_uv, at);
    v_uv_p = region(u_uv_p, at);
}
