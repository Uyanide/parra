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

in vec2 v_uv;
in vec2 v_uv_p;

out vec4 fragColour;

// Both branches are on uniforms, so every fragment takes the same path and the
// common case costs exactly one fetch.
vec3 layer(sampler2D sharp, sampler2D blurred, vec2 uv) {
    vec3 colour = texture(sharp, uv).rgb;
    if (u_blur <= 0.0) {
        return colour;
    }
    return mix(colour, texture(blurred, uv).rgb, u_blur);
}

void main() {
    vec3 colour = layer(u_sharp, u_blurred, v_uv);
    if (u_mix < 1.0) {
        colour = mix(layer(u_sharp_p, u_blurred_p, v_uv_p), colour, u_mix);
    }
    colour = mix(colour, u_tint.rgb, u_tint.a * u_blur);
    fragColour = vec4(colour, 1.0);
}
