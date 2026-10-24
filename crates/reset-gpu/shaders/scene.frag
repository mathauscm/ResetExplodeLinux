#version 460
// Pixels da cena de jogo (port do HLSL do GpuGameBench.cs). Variantes por define: TERRAIN, TREE, WATER, SKY e POST.
// Monocromático como no app do Windows: o visual preto e branco é o do RESET EXPLODE.
#include "scene_common.glsl"

layout(set = 0, binding = 2) uniform sampler2DShadow shadowMap;
layout(set = 0, binding = 3) uniform sampler2D hdrTex;

#if defined(SKY) || defined(POST)
layout(location = 0) in vec2 vUv;
#else
layout(location = 0) in vec3 vWpos;
layout(location = 1) in vec3 vNrm;
layout(location = 2) in float vMat;
#endif

layout(location = 0) out vec4 outColor;

vec3 sky(vec3 dir) {
    float up = clamp(dir.y, 0.0, 1.0);
    vec3 col = mix(vec3(0.42), vec3(0.12), pow(up, 0.6));
    float sun = clamp(dot(dir, -F.sunDir.xyz), 0.0, 1.0);
    col += pow(sun, 600.0) * 12.0 + pow(sun, 12.0) * 0.25;
    return col;
}

// Sombra com PCF 5x5 (25 comparações filtradas pelo hardware, como o SampleCmpLevelZero do D3D).
float shadow(vec3 wpos, vec3 n) {
    vec4 lp = F.lightViewProj * vec4(wpos + n * 0.15, 1.0);
    vec2 uv = lp.xy * 0.5 + 0.5;
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) return 1.0;
    float sum = 0.0;
    for (int y = -2; y <= 2; y++)
        for (int x = -2; x <= 2; x++)
            sum += texture(shadowMap, vec3(uv + vec2(x, y) / 4096.0, lp.z - 0.0015));
    return sum / 25.0;
}

vec3 shade(vec3 wpos, vec3 n, float albedo) {
    vec3 l = -F.sunDir.xyz;
    float ndl = clamp(dot(n, l), 0.0, 1.0);
    vec3 direct = vec3(ndl * shadow(wpos, n) * 1.5);
    vec3 ambient = vec3(mix(0.06, 0.16, n.y * 0.5 + 0.5));
    vec3 col = albedo * (direct + ambient);
    float dist = length(wpos - F.camTime.xyz);
    float fog = 1.0 - exp(-dist * 0.0022);
    return mix(col, sky(normalize(wpos - F.camTime.xyz)), fog);
}

void main() {
#if defined(TERRAIN)
    vec3 n = normalize(vNrm);
    float slope = 1.0 - n.y;
    float h = vWpos.y;
    float albedo = mix(0.22, 0.15, clamp((h - 8.0) / 25.0, 0.0, 1.0));                                // grama
    albedo = mix(albedo, 0.28, clamp((slope - 0.25) * 4.0, 0.0, 1.0));                                // rocha nas encostas
    albedo = mix(albedo, 0.70, clamp((h - 62.0) / 10.0, 0.0, 1.0) * clamp(1.0 - slope * 3.0, 0.0, 1.0)); // neve nos picos
    albedo *= 0.9 + 0.1 * fract(sin(dot(floor(vWpos.xz * 2.0), vec2(12.9898, 78.233))) * 43758.5453);
    outColor = vec4(shade(vWpos, n, albedo), 1.0);
#elif defined(TREE)
    float albedo = vMat < 1.5 ? 0.08 : 0.11;
    outColor = vec4(shade(vWpos, normalize(vNrm), albedo), 1.0);
#elif defined(WATER)
    vec2 p = vWpos.xz;
    vec3 n = vec3(0.0, 1.0, 0.0);
    for (int k = 0; k < 6; k++) {   // ondas somadas
        vec2 d = vec2(cos(float(k) * 1.7), sin(float(k) * 1.7));
        float f = 0.15 * float(k + 1);
        float ph = dot(d, p) * f + F.camTime.w * (1.0 + float(k) * 0.4);
        n.xz += d * cos(ph) * 0.06 / float(k + 1);
    }
    n = normalize(n);
    vec3 v = normalize(vWpos - F.camTime.xyz);
    float fres = 0.04 + 0.96 * pow(1.0 - clamp(dot(-v, n), 0.0, 1.0), 5.0);
    vec3 refl = sky(reflect(v, n)) * shadow(vWpos, n);
    vec3 col = mix(vec3(0.05), refl, fres);
    float fog = 1.0 - exp(-length(vWpos - F.camTime.xyz) * 0.0022);
    outColor = vec4(mix(col, sky(v), fog), 1.0);
#elif defined(SKY)
    vec4 w = F.invViewProj * vec4(vUv * 2.0 - 1.0, 1.0, 1.0);
    outColor = vec4(sky(normalize(w.xyz / w.w - F.camTime.xyz)), 1.0);
#elif defined(POST)
    // HDR → tela: tone mapping ACES, vinheta e gama.
    vec3 c = texture(hdrTex, vUv).rgb;
    c = c * (2.51 * c + 0.03) / (c * (2.43 * c + 0.59) + 0.14);
    vec2 d = vUv - 0.5;
    c *= 1.0 - dot(d, d) * 0.9;
    float g = pow(clamp(c.r, 0.0, 1.0), 1.0 / 2.2);
    outColor = vec4(g, g, g, 1.0);
#endif
}
