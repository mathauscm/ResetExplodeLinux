#version 460
// Pixel do pelo: cadeia de FMAs vetoriais (o que mais consome energia na GPU) antes do descarte e ligada a ele,
// para o compilador não poder pular o cálculo. Pontas translúcidas (alpha blending soma carga de ROP e banda).
layout(std140, set = 0, binding = 0) uniform CB {
    mat4 world;
    mat4 viewProj;
    vec4 p;   // x = tempo, y = número de camadas, z = comprimento do pelo, w = iterações de ALU
} C;

layout(location = 0) in vec3 vNrm;
layout(location = 1) in vec2 vUv;
layout(location = 2) in float vShell;

layout(location = 0) out vec4 outColor;

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

void main() {
    float h = hash(floor(vUv * vec2(900.0, 380.0)));

    vec4 a = vec4(h, vUv, vShell), b = a.wzyx + 0.5;
    int n = int(C.p.w);
    for (int k = 0; k < n; k++) {
        a = fma(a, vec4(1.0001), b * 0.0003);
        b = fma(b, vec4(0.9997), a * 0.0002);
    }
    float acc = fract(dot(a, b));

    if (vShell > 0.0 && h + acc * 1e-6 < vShell) discard;   // fios afinam até a ponta

    vec3 L = normalize(vec3(-0.4, 0.8, -0.5));
    float diff = clamp(dot(normalize(vNrm), L), 0.0, 1.0) * 0.75 + 0.25;
    float ao = mix(0.18, 1.0, vShell);                       // raiz escura, ponta clara
    float c = diff * ao * (0.9 + 0.1 * acc);
    outColor = vec4(c, c, c, mix(1.0, 0.55, vShell));
}
