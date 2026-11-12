//! Cursor shape is part of both the painted geometry and retained damage state.

use alacritty_terminal::vte::ansi::CursorShape;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub column: u32,
    pub row: u32,
    pub shape: CursorShape,
}

impl Cursor {
    /// Physical pixel rectangles, with a minimum two-pixel stroke for visibility.
    pub fn rects(self, cell_w: f32, cell_h: f32, strip_h: f32) -> impl Iterator<Item = [f32; 4]> {
        let x = self.column as f32 * cell_w;
        let y = strip_h + self.row as f32 * cell_h;
        let stroke = (cell_w * 0.15).max(2.0).min(cell_w * 0.5).min(cell_h * 0.5);
        let mut rects = [[x, y, cell_w, cell_h]; 4];
        let count = match self.shape {
            CursorShape::Hidden => 0,
            CursorShape::Block => 1,
            CursorShape::Beam => {
                rects[0][2] = stroke;
                1
            }
            CursorShape::Underline => {
                rects[0] = [x, y + cell_h - stroke, cell_w, stroke];
                1
            }
            CursorShape::HollowBlock => {
                rects = [
                    [x, y, cell_w, stroke],
                    [x, y + cell_h - stroke, cell_w, stroke],
                    [x, y + stroke, stroke, cell_h - 2.0 * stroke],
                    [
                        x + cell_w - stroke,
                        y + stroke,
                        stroke,
                        cell_h - 2.0 * stroke,
                    ],
                ];
                4
            }
        };
        rects.into_iter().take(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn application_sequences_change_geometry_without_moving_cursor() {
        let mut term = Term::new(Config::default(), &TermSize::new(40, 10), VoidListener);
        let mut parser: Processor = Processor::default();
        parser.advance(&mut term, b"\x1b[3;4H");
        let mut previous = None;
        // DECSCUSR: block in normal mode, beam in insert, underline in replace,
        // and back to the default block on exit. Test both blink variants.
        for (sequence, expected) in [
            (&b"\x1b[2 q"[..], [30.0, 60.0, 10.0, 20.0]),
            (&b"\x1b[5 q"[..], [30.0, 60.0, 2.0, 20.0]),
            (&b"\x1b[4 q"[..], [30.0, 78.0, 10.0, 2.0]),
            (&b"\x1b[6 q"[..], [30.0, 60.0, 2.0, 20.0]),
            (&b"\x1b[0 q"[..], [30.0, 60.0, 10.0, 20.0]),
        ] {
            parser.advance(&mut term, sequence);
            let content = term.renderable_content();
            let cursor = Cursor {
                column: content.cursor.point.column.0 as u32,
                row: content.cursor.point.line.0 as u32,
                shape: content.cursor.shape,
            };
            assert_eq!(
                cursor.rects(10.0, 20.0, 20.0).collect::<Vec<_>>(),
                [expected]
            );
            // The same comparison drives overlay invalidation for an idle cell.
            assert_ne!(Some(cursor), previous);
            previous = Some(cursor);
        }
    }

    #[test]
    fn hidden_and_hollow_cursors() {
        let cursor = Cursor {
            column: 0,
            row: 0,
            shape: CursorShape::Hidden,
        };
        assert_eq!(cursor.rects(10.0, 20.0, 0.0).count(), 0);
        let cursor = Cursor {
            shape: CursorShape::HollowBlock,
            ..cursor
        };
        assert_eq!(
            cursor.rects(10.0, 20.0, 0.0).collect::<Vec<_>>(),
            [
                [0.0, 0.0, 10.0, 2.0],
                [0.0, 18.0, 10.0, 2.0],
                [0.0, 2.0, 2.0, 16.0],
                [8.0, 2.0, 2.0, 16.0],
            ]
        );
    }
}
