#version 300 es
precision highp float;

uniform sampler2D u_source;
// Half a texel of the destination, in normalized coordinates. The destination is half
// the size of the source, so at offset 1 the diagonal taps land one source texel out.
uniform vec2 u_halfpixel;
uniform float u_offset;

in vec2 v_uv;

out vec4 fragColour;

// Dual-Kawase downsample: the centre plus four diagonals, weighted 4:1:1:1:1.
//
// Alpha is carried with the colour. The source is premultiplied, so averaging the four
// components together is the same operation the three used to be.
void main() {
    vec2 o = u_halfpixel * u_offset;
    vec4 sum = texture(u_source, v_uv) * 4.0;
    sum += texture(u_source, v_uv - o);
    sum += texture(u_source, v_uv + o);
    sum += texture(u_source, v_uv + vec2(o.x, -o.y));
    sum += texture(u_source, v_uv - vec2(o.x, -o.y));
    fragColour = sum / 8.0;
}
