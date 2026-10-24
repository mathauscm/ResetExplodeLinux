// Dados por frame da cena de jogo, comuns a todos os shaders dela (mesmo layout de FrameUbo em scene.rs).
layout(std140, set = 0, binding = 0) uniform Frame {
    mat4 viewProj;
    mat4 lightViewProj;
    mat4 invViewProj;
    vec4 camTime;   // xyz = posição da câmera, w = tempo em segundos
    vec4 sunDir;    // xyz = direção em que a luz do sol viaja
} F;
