#version 460
// Vértices da cena de jogo. Um arquivo, cinco shaders (escolhidos por define no build.rs):
// TERRAIN, TREE (instancing), WATER, SHADOW_TERRAIN e SHADOW_TREE (mapa de sombras).
#include "scene_common.glsl"

// Árvores: xyz = posição no chão, w = escala.
layout(std430, set = 0, binding = 1) readonly buffer Trees { vec4 trees[]; };

layout(location = 0) in vec3 inPos;
layout(location = 1) in vec3 inNrm;
layout(location = 2) in float inMat;

#if !defined(SHADOW_TERRAIN) && !defined(SHADOW_TREE)
layout(location = 0) out vec3 vWpos;
layout(location = 1) out vec3 vNrm;
layout(location = 2) out float vMat;
#endif

// Cada árvore com giro próprio, escala e balanço de vento.
vec3 treeWorld(vec3 p, uint iid, out mat3 rot) {
    vec4 t = trees[iid];
    float a = fract(sin(float(iid) * 12.9898) * 43758.5453) * 6.2831;
    float c = cos(a), s = sin(a);
    rot = mat3(c, 0.0, -s, 0.0, 1.0, 0.0, s, 0.0, c);
    float sway = sin(F.camTime.w * 1.3 + float(iid)) * 0.03 * p.y;
    return rot * p * t.w + t.xyz + vec3(sway, 0.0, sway * 0.5);
}

void main() {
#if defined(TERRAIN)
    vWpos = inPos;
    vNrm = inNrm;
    vMat = inMat;
    gl_Position = F.viewProj * vec4(inPos, 1.0);
#elif defined(TREE)
    mat3 rot;
    vWpos = treeWorld(inPos, uint(gl_InstanceIndex), rot);
    vNrm = rot * inNrm;
    vMat = inMat;
    gl_Position = F.viewProj * vec4(vWpos, 1.0);
#elif defined(WATER)
    vWpos = inPos;
    vNrm = vec3(0.0, 1.0, 0.0);
    vMat = 0.0;
    gl_Position = F.viewProj * vec4(inPos, 1.0);
#elif defined(SHADOW_TERRAIN)
    gl_Position = F.lightViewProj * vec4(inPos, 1.0);
#elif defined(SHADOW_TREE)
    mat3 rot;
    gl_Position = F.lightViewProj * vec4(treeWorld(inPos, uint(gl_InstanceIndex), rot), 1.0);
#endif
}
