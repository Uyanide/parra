#version 300 es
precision highp float;

uniform sampler2D u_sharp;
uniform sampler2D u_blurred;
uniform sampler2D u_sharp_p;
uniform sampler2D u_blurred_p;

// 0 shows the sharp texture, 1 the baked blurred one.
uniform float u_blur;
// 0 shows the outgoing wallpaper, 1 the current one.
uniform float u_mix;
// Tint colour, with the configured opacity already folded into the alpha.
uniform vec4 u_tint;
// How much of the frame is there at all. Below 1 while a wallpaper is arriving or leaving.
uniform float u_opacity;

in vec2 v_uv;
in vec2 v_uv_p;

out vec4 fragColour;

// Both branches are on uniforms, so every fragment takes the same path and the
// common case costs exactly one fetch.
vec4 layer(sampler2D sharp, sampler2D blurred, vec2 uv) {
    vec4 colour = texture(sharp, uv);
    if (u_blur <= 0.0) {
        return colour;
    }
    return mix(colour, texture(blurred, uv), u_blur);
}

// Every texture here is premultiplied, so both mixes below are linear in colour and
// coverage alike and the weights are the same ones an opaque frame used.
void main() {
    vec4 colour = layer(u_sharp, u_blurred, v_uv);
    if (u_mix < 1.0) {
        colour = mix(layer(u_sharp_p, u_blurred_p, v_uv_p), colour, u_mix);
    }
    // Scaled by the coverage it lands on, so it tints the wallpaper and keeps rgb within
    // alpha, which is the invariant the compositor reads the buffer under.
    colour.rgb = mix(colour.rgb, u_tint.rgb * colour.a, u_tint.a * u_blur);
    // Premultiplied, so scaling all four channels together is what makes the frame weigh
    // less without changing its colour or leaving rgb outside its alpha.
    if (u_opacity < 1.0) {
        colour *= u_opacity;
    }
    fragColour = colour;
}
