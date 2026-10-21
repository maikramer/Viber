# Consolas — fora desta geração

O Viber não vai a PlayStation, Xbox nem Switch neste roadmap. O Bevy 0.20
não tem um caminho de certificação (TRC / XR / Lotcheck) e não há alvo de
publicação. Retomar só com esse alvo.

O que bloqueia, por ordem:

1. **SDK e devkit.** Cada plataforma exige o SDK fechado do fabricante, uma
   máquina de desenvolvimento e uma conta de publisher. Nada disto está no
   repo nem no `cargo build` de desktop.
2. **Save.** O jogo grava em `~/.local/share/viber` (ou `save.dir` do
   `config.yaml`). Nas consolas o save é um slot da plataforma, com quota e
   diálogo de sistema. O `src/save.rs` teria de deixar de assumir um caminho
   POSIX.
3. **Shaders.** O pipeline é Vulkan + WGSL via WESL. Consolas compilam HLSL
   ou PSSL offline, com um conjunto de features diferente (sem `wgpu` tal
   como está, sem DLSS da NVIDIA). Os materiais bindless do terreno
   (`src/terrain/layer_material.rs`) teriam de ter um ramo não-bindless.
4. **Input.** O mapa de ações (`src/input_map.rs`) já separa teclado e
   gamepad, mas o gamepad de comando não é um controle HID: é a API da
   plataforma, com utilizador, vibração e layouts certificados.
5. **TRC / XR.** Capturas de ecrã obrigatórias, suspensão da aplicação,
   utilizador que muda a meio do jogo, trophies/achievements, e uma lista
   de falhas que rejeita o binário. Não há harness para isso.

Até lá o alvo continua a ser PC (Linux/Vulkan, Windows via o mesmo binário
Bevy) em single-player.
