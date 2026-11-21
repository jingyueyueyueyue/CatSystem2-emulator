# PC state-route candidate: 04 Yukikoi Melt

2026-11-20. This is a route contract, not a runnable `--scenarios` input. Do not schedule either Enter by frame. Use `pc_story_04_wipe_no_key_probe.json` for the guarded normal entry and observation with no body input. The 06:07 build cannot authorize these state steps.

## Identity and evidence

- Original `YukikoiMelt.exe` SHA-256: `63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976`; startup `kcs.int/main.kcs`. `scene.int::__tar01.cst` SHA-256: `7acf10b32a2184fbadd42544b01b8b0eeb0254ad109dd9bb653a80c67102922d`; no CST override in `update00.int`. The observed dialogue is Japanese; other language selections are not qualified.
- Evidence directory: `F:/CS2-build-cache/playthrough/2026-11/20261120_060957_411669_uom5538o_playthrough`. `04.events.log:4403` and `:5091` show the two wipes halting naturally; `:5110` shows `326` at abs `53` then `54`, next to the nonempty `[message]` and `[input]` at f852. `04.host.log:329` shows `meswnd.fes` at `MES_SHOW` with `WAIT_KEY=true`, no pending messages, and no active movie. The same wait was observed at f1080 and f1320.
- `trace_enabled=false`, `profile=None`, `body_generation=None`. The `str[0]` layer already reports 4808 alpha pixels before the message runs. That count, the wait icon, `[input]`, and native `71` do not identify a fresh composed page.

## Exact CST positions

Group-local `r` and absolute CST indices are separate. The state-route `record` uses the absolute index. Each wait is a `0x02` marker; each body below is a nonempty `0x20` record from the original member.

| Step | Body | Wait before input | Action and required successor |
| --- | --- | --- | --- |
| Verify first advance | `g0/r53`, abs `53` | `page / __tar01 / 0 / 54` (`g0/r54`) | One Enter (`13`) after the gate below. Observe `g1/r7 rdraw 15` (abs `62`) and `r8 wait` retire naturally, then `g1/r9 pcm SHI_tar01_001` (abs `64`), new body `g1/r11` (abs `66`), and committed `page / __tar01 / 1 / 67` (`g1/r12`). Stop without a second key on this run. |
| Verify second advance | `g1/r11`, abs `66` | `page / __tar01 / 1 / 67` (`g1/r12`) | Only after the first advance is verified, repeat the gate and send one Enter. The next group has a speaker change (abs `68`), a new line `g2/r1` (abs `69`), and committed `page / __tar01 / 2 / 70` (`g2/r2`). Stop at `70`, with no third key. |

## Input gate and ownership

1. Require the pinned identity, current `__tar01` context, and exact marked wait. Require one foreground `meswnd.fes` receiver at `MES_SHOW` / `WAIT_KEY=true`, a focused, unpaused VM, and no pending input/messages, blocking transition/movie, auto-advance, title, menu, or choice. A timer or the earlier person-card wait never qualifies.
2. Require a fresh body generation tied to the current scene load, expected `0x20` body and marked wait. The reveal must be complete and the matching generation committed to the visible body surface with nonzero glyph alpha. For manual review only, a single final-composition image can corroborate the actual line if logs cannot identify it. Missing generation, blank or clipped glyphs, and unresolved identity leave automation observation-only.
3. Require a key receipt and progress to the expected CST successor, with a different, newly committed nonempty body and a new matching wait. A generic receipt or timeout without the successor is not success. Keep `page_scenes` empty, use a single explicit action per verified stage, and stop on a mismatch without a retry key.

Averroes owns the shared trace/profile and route runner. This file supplies only the 04 identity, stage contract, and negative cases. Do not reuse another game's `page` classifier. Verify and merge the exact profile gate before turning these stages into a runnable keyed route.

## Adjacent samples: 06 and 07

- **06**: Reuse `pc_story_06_07_entry_observe.json` for the guarded f530 and f660 entry clicks. At f1440, `20261120_054848_211446_zbe8beaf_playthrough/06.host.log:127` agrees with `C01_01 g0/r65` waiting. The real initial body is abs `61`; abs `64` is an empty `0x20`, not a new displayed page. The historical single advance received Enter at f1421 and reached `frameoff fade 30`, `wait 210`, `pcm C_IROHA_C01_001`, new body `g1/r32` (abs `98`) at f1762, and `[input]` `g1/r41` (abs `107`) at f1765 (`20261120_034451_363895_n4klau1__playthrough/06.events.log:12352` and `:17818`). Verify the current build's final composition at both waits; the old command trace is not an input schedule. No stuck 527/530 is established on this segment.
- **07**: Run `pc_story_07_title_hit_observe.json` in one slot. It only moves the pointer and inspects. Require masked region `20` enabled at `(176,256)` on the settled title. The newer run stopped with `input_missed` because f840 actually matched full-screen region `31`; it did not enter `com01`. The older run hit `20` and reached `com01 / wait`, but its Enter at f1600 yielded only an input receipt, with no new body before its frame cap. Once the title gate is re-proven, allow one New Game click and let the scene's configured movie/intro and timers run naturally. Original `scene.int::com01.cst` (SHA-256 `bccd3463db8d0f173101609a1ad6e57f4edc5d7f2c084763a17826863d8ce086`) puts the first nonempty dialogue at `g0/r56`, then the first body wait at `g0/r57`. Stop at any distinct pre-body input or unexpected region. The historical `DIFIMAGE face attr` error is a different path, not confirmed on the current candidate.
- **07 current title evidence**: `20261120_062045_718254_qofyap_8_playthrough/07.host.log:32` shows `flow/WAIT_TITLE`, `ttlflow/FES_WAIT`, and `title/IN` at f840; `:62`, `:92`, and `:122` show `title/WAIT` at f1120, f1600, and f2100, with no active movie. Only f805 was sampled for masked hits in that run. The new probe checks hit identity inside the observed `WAIT` interval; `title/WAIT` alone does not authorize a click.
