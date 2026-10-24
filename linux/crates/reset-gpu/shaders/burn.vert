#version 460
// Stress de GPU (port do GpuBurn.cs): o torus é desenhado N vezes (uma instância por camada de pelo),
// cada camada inflada ao longo da normal, com gravidade e vento.
layout(std140, set = 0, binding = 0) uniform CB {
    mat4 world;
    mat4 viewProj;
    vec4 p;   // x = tempo, y = número de camadas, z = comprimento do pelo, w = iterações de ALU
} C;

layout(location = 0) in vec3 inPos;
layout(location = 1) in vec3 inNrm;
layout(location = 2) in vec2 inUv;

layout(location = 0) out vec3 vNrm;
layout(location = 1) out vec2 vUv;
layout(location = 2) out float vShell;

void main() {
    float s = float(gl_InstanceIndex) / C.p.y;                // 0 = raiz, 1 = ponta
    vec3 p = inPos + inNrm * (C.p.z * s);
    vec3 w = (C.world * vec4(p, 1.0)).xyz;
    w.y -= 0.10 * s * s;                                       // gravidade
    w += 0.025 * s * sin(C.p.x * 2.7 + inPos.yzx * 7.0);       // vento
    gl_Position = C.viewProj * vec4(w, 1.0);
    vNrm = normalize((C.world * vec4(inNrm, 0.0)).xyz);
    vUv = inUv;
    vShell = s;
}
