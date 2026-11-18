**Layout and capacity ownership audit — 2026-11-18**

Implementation follow-up: the findings below describe the pre-repair snapshot;
line references belong to that snapshot. The fixes now use a dynamically grown
text pool, measured graveyard/settings/close footers, naturally laid out history
details, component-owned switcher/carousel geometry, actual font measurements,
and semantic control bounds for graphical tests. The switcher fits a minimum
two-column grid beside the workspace rail when space permits, including large
3840-wide previews. See [layout ownership](ui_style.md#layout-ownership) for the
current contracts and [regression checks](../tests/README.md) for verification.

Reviewed the active Rust UI, window controller, font/rendering code, and associated tests for guessed geometry, duplicated calculations, and capacity assumptions crossing component boundaries. This is a focused audit of that pattern, not a complete correctness review of the terminal or vendored dependencies. Production sources were left unchanged by the audit. Layout probes ran against a copy of `src` in `/tmp/mypixels-layout-audit-r68qr99e`; the renderer reproduction used the project's debug binary on a private Xvfb display.

The recent 2×2 switcher change fixes the requested minimum size but leaves its underlying layout ownership problem in place. Renaming the numeric literals would not fix that problem.

1. **P1 — A fixed text-buffer pool silently limits valid terminal output. Confirmed by rendering.**

   [main.rs:54](../src/main.rs#L54) defines `MAX_RUNS = 4096`. [gpu.rs:671](../src/gpu.rs#L671) allocates that many buffers, and the live renderer at [main.rs:3006](../src/main.rs#L3006) uses `.take(MAX_RUNS)`. Both background-preview paths repeat the truncation at lines 3105 and 3180. Adjacent cells with different foregrounds legitimately produce distinct runs; their number depends on terminal contents, not just a typical line count.

   An isolated 120-column × 51-row terminal was filled with 6,120 alternating red/green `X` characters, with full redraw enabled. Pixel inspection found exactly **4,096 glyphs**: 34 complete rows, 16 glyphs in the next row, and 16 completely blank rows. Screenshot and measurements: `/tmp/mypixels-run-cap-audit-vctibqtp/run-cap.png` and `result.json`. Reproduction driver: `/tmp/mypixels-renderer-cap-audit.py`.

   The GPU buffer pool should grow to the actual prepared run count before references into it are created, or rendering should batch all runs. Its allocation policy must not decide which terminal content exists. The current unsafe-reference approach also needs to keep growth outside the borrowing/drawing phase.

2. **P2 — Graveyard details are allocated after a list that consumes all remaining height. Confirmed with egui geometry.**

   The list at [egui_ui.rs:1081](../src/egui_ui.rs#L1081) uses `auto_shrink([false, false])` with no height reserved for subsequent content. The selected-tab details then allocate another 210 points at [egui_ui.rs:1194](../src/egui_ui.rs#L1194).

   With one selected buried tab in a 1500×900 viewport, the details' “Raise tab” label is at y=1020.5 and “Kill for good” at y=1066.5, outside the clip. At 560×480 they are at y=600.5 and y=646.5. Row controls and keyboard actions remain separate ways to act; the selected-details controls are inaccessible in these cases.

   The panel should compose a bounded details section and a list in the remaining space, or put both inside a single appropriate scroll container. Appending content after a fill-remaining-space child cannot provide that allocation.

3. **P2 — Switcher sizing, drawing, and keyboard navigation share assumptions rather than a layout result. Confirmed geometry drift.**

   [egui_ui.rs:2185](../src/egui_ui.rs#L2185) independently constructs the popup's outer size using `36`, `180`, and `136`. Here `36` is twice the frame's 18-point padding. `136` reconstructs header 40 + gap 12 + gap 18 + footer 30 + padding 36. `180` supplies a combined vertical allowance, leaving a nominal 44 points outside the popup after adding 136 back.

   But [egui_ui.rs:2344](../src/egui_ui.rs#L2344) draws either a fixed 30-point compact footer or a naturally sized row of keycaps and labels. The frame has its own stroke as well. Probes measured **682 requested versus 678 painted points** at 1500×900, and **436 requested versus 438 painted points** at 560×480. This is actual disagreement between the size model and the rendered component.

   [main.rs:2427](../src/main.rs#L2427) computes display dimensions while [main.rs:4648](../src/main.rs#L4648) independently reconstructs window dimensions, strip height, card width and column count for keyboard navigation. Helpers are shared, but each caller still knows how to reconstruct the other's inputs.

   One switcher component should own the minimum 2×2 policy, measured header/footer, frame margins and constrained grid viewport. It should publish the resulting columns and card geometry for navigation and hit testing. Preview texture availability should remain independent of this layout.

4. **P2 — History's parent guesses its children's combined height. Design risk confirmed in source.**

   [egui_ui.rs:691](../src/egui_ui.rs#L691) sizes the command list with `(available_height - 550).clamp(76, 274)`. The details child separately allocates 368 points at [egui_ui.rs:871](../src/egui_ui.rs#L871), using offsets such as 56, 170 and 42 internally. The output preview separately subtracts 50 points at line 834 for its own frame/header.

   There is no contract relating 550 to those actual child sizes. Adding an action, changing font metrics or changing a child margin requires finding the parent's reservation too. The outer scroll area mitigates clipping; this finding is about brittle ownership rather than a demonstrated missing control.

   Details should report their measured extent. The list and preview should receive explicit remaining-space allocations according to a panel policy, without an aggregate guessed allowance.

5. **P2 — Settings and close-dialog parents duplicate footer knowledge. Design risk confirmed in source.**

   Settings reserves 86 points at [egui_ui.rs:2595](../src/egui_ui.rs#L2595), then independently draws spacing, a separator, explanatory text, and `actions()` at lines 2607–2610. Its parent must know the combined height of those children.

   The close dialog calculates a footer height of 88/136/40 at [egui_ui.rs:1738](../src/egui_ui.rs#L1738), subtracts that and another 110 points from the screen to constrain details, then repeats the footer branch at [egui_ui.rs:1782](../src/egui_ui.rs#L1782). The tested 480×320 and 560×480 close dialogs kept their action labels visible; no failure is claimed for those cases.

   In each case, the footer should own its layout once. The container should allocate the body from the footer's measured extent, using egui layout/containers or a single component-owned layout result.

6. **P2 — Workspace parents anticipate carousel internals. Design risk confirmed in source, including the new switcher rail.**

   The workspace panel computes preview widths from `(available_width - 40) / 3` at [egui_ui.rs:1375](../src/egui_ui.rs#L1375), then separately builds a preview rectangle inset by 12 points per side at line 1437. Compact callers subtract 34 before dividing by two at lines 1411 and 2424. The carousel at [egui_ui.rs:1478](../src/egui_ui.rs#L1478) subsequently decides its own gap, overflow and arrow gutters, which may be 0, 10, 14 or 24 points per side.

   Those callers are effectively estimating the carousel's chrome without owning its overflow decision. A carousel should accept a rectangle and a presentation policy (preferred tile size/count, compact/expanded), then own gutters, tile sizing, scrolling and hit geometry. The caller should not reverse-engineer those details.

7. **P2 — Tests repeat implementation geometry and make the coupling harder to remove. Confirmed in source.**

   [workspace_panel_smoke.py:127](../tests/workspace_panel_smoke.py#L127) reproduces panel width and then expressions such as `24 + 14 + 36 + 8 + 2 * 76 + 64` to locate previews. [switcher_smoke.py:224](../tests/switcher_smoke.py#L224), added in this session, assumes 90-point cards, an 8-point gap and additional internal offsets. The new switcher unit test also repeats the 180-point reservation at [egui_ui.rs:4127](../src/egui_ui.rs#L4127).

   This means layout changes require synchronized edits to tests, and some checks reinforce the same assumptions they should challenge. The screenshot comparison and terminal-input assertions are useful independent checks and should remain.

   Tests should locate controls by semantic identity and actual published/accessibility rectangles, then assert observable outcomes: no clipped actions, no overlap, stable popup bounds, correct selected terminal, and complete rendered content. The settings navigation already logs actual button geometry at lines 2584–2589; that is a useful precedent, though a reusable semantic geometry interface would be cleaner.

8. **P3 — Text width is sometimes estimated despite a measured-text helper. Confirmed in source.**

   History reserves sequence-number width using digit count × 8.5 at [egui_ui.rs:722](../src/egui_ui.rs#L722). Compact tab badges use character count × 7 at [tab_bar.rs:136](../src/tab_bar.rs#L136). These approximate metrics of the fonts used by the drawing functions. [ui_style.rs:247](../src/ui_style.rs#L247) already demonstrates measuring the actual galley.

   Text components should measure and draw the same galley/font. Padding remains an explicit design choice; glyph advances should come from the font backend.

**Repair order and boundary rule**

Fix the reproduced output truncation and unreachable graveyard details first. Then replace the switcher's duplicated size model, followed by shared body/footer composition and carousel ownership. Update test targeting alongside those component changes rather than preserving numeric layout copies.

The useful existing precedent is `tab_bar::Geometry`: the strip returns the rectangles it actually allocated, and the controller consumes them. Extend that ownership pattern instead of teaching controllers and tests how to calculate another component's geometry.

Ordinary spacing/radius choices, documented resource limits, protocol offsets, color conversion, clip-space mathematics and the font metric parser were not flagged merely for containing numbers. The criterion is whether an independently maintained assumption stands in for an authoritative component result.
