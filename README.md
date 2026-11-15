# Plano

Planificateur de plans et de tâches, open source, léger, 100 % hors ligne (Rust).
Spécification complète : `docs/SPEC.md` (à copier depuis le document de conception).

## Démarrage

```bash
cargo test --workspace
cargo run -p plano-cli -- plan add "Sport"
cargo run -p plano-cli -- plan list
cargo run -p plano-cli -- task add <ID_DU_PLAN> "Séance jambes"
cargo run -p plano-cli -- task list <ID_DU_PLAN>
cargo run -p plano-cli -- task done <ID_DE_LA_TACHE>
```

Licence : MIT OR Apache-2.0.
