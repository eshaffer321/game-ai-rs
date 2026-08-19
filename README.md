# game-ai-rs

A reusable Rust chassis for deterministic, alternating, two-player,
zero-sum, perfect-information game AI, extracted from
[onitama](https://github.com/)'s alpha-beta engine (`onitama-ai`) once
a second real rules engine ([santorini-rs](https://github.com/)'s
`santorini-core`) existed to shape the interface against.

See `DESIGN.md` for the full rationale: what the `Game` trait covers,
what it deliberately leaves out, and the staged migration plan for the
rest of Onifish's search machinery.

## Crates

- `game-ai-core` -- the `Game` trait. No dependencies on any specific
  game.
- `game-ai-alphabeta` -- iterative-deepening negamax generic over
  `G: Game, H: SearchHooks<G>`, mechanically ported from Onifish's
  original engine.
- `game-ai-arena` -- game-agnostic self-play/match tooling generic
  over `G: Game` (not yet built).

Game-specific adapters live in each game's own repository, not here --
see DESIGN.md's "Repo layout" section.

## Building

`cargo test` at the workspace root. This crate has no dependency on
either game repository; the dependency runs the other way (each game
repository depends on this one via a pinned Git revision).
