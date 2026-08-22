#version 300 es
precision highp float;

uniform sampler2D u_source;
// Half a texel of the destination, which here is twice the size of the source.
uniform vec2 u_halfpixel;
uniform float u_offset;

in vec2 v_uv;

out vec4 fragColour;

// Dual-Kawase upsample: four axis taps at weight 1 and four diagonals at weight 2.
// Alpha is carried with the colour, for the reason given in the downsample.
void main() {
    vec2 o = u_halfpixel * u_offset;
    vec4 sum = texture(u_source, v_uv + vec2(-o.x * 2.0, 0.0));
    sum += texture(u_source, v_uv + vec2(-o.x, o.y)) * 2.0;
    sum += texture(u_source, v_uv + vec2(0.0, o.y * 2.0));
    sum += texture(u_source, v_uv + vec2(o.x, o.y)) * 2.0;
    sum += texture(u_source, v_uv + vec2(o.x * 2.0, 0.0));
    sum += texture(u_source, v_uv + vec2(o.x, -o.y)) * 2.0;
    sum += texture(u_source, v_uv + vec2(0.0, -o.y * 2.0));
    sum += texture(u_source, v_uv + vec2(-o.x, -o.y)) * 2.0;
    fragColour = sum / 12.0;
}
