# Pool mínimo do simple-racer

Subset local para a demo correr sem o checkout do Vael ao lado.
`examples/simple-racer/config.yaml` aponta para `public/` daqui.

Inclui os GLBs citados pelo XML da pista, as layers de terreno que o
`<Terrain>` nomeia, o BGM `race` e os SFX de veículos.

Manifests que regeneram **aqui** (output em `public/assets`):

```bash
cd examples/racer-assets
gameassets resume --profile game.yaml --manifest manifests/vehicles
gameassets resume --profile game.yaml --manifest manifests/audio-sfx-vehicles
```

Árvores, casas e props de vila citados pela pista são um snapshot dos
ficheiros do Vael (`../Vael/assets`). A receita desses packs fica no jogo.
