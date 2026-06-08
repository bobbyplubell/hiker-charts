//! Read-only "Data" pane: the source table as the engine parses it.
//!
//! Shows one column per data column with its *inferred* type (the same `infer_type` the
//! resolver uses), and each cell rendered through the shared `coerce` + `format_value` path
//! so dates appear as dates, numbers normalized, and unparseable/empty cells as a muted em
//! dash. Purely a viewer — it never mutates `BuilderState` (SPEC: data is read-only input).

use hiker_charts_core::dsl::DataType;
use hiker_charts_core::display::format_value;
use hiker_charts_core::typing::{coerce, infer_type, Value};

use crate::model::BuilderState;

/// Render the builder's resolved table as a scrollable, read-only grid. Each header carries
/// the column's inferred type; each cell shows its coerced/formatted value.
pub fn data_view(state: &BuilderState, ui: &mut egui::Ui) {
    let table = state.table();
    if table.columns.is_empty() {
        ui.weak("no data");
        return;
    }
    // Infer each column's type once; reused for every cell in that column.
    let types: Vec<DataType> = table.columns.iter().map(|c| infer_type(&c.cells)).collect();
    let rows = table.row_count();
    let weak = ui.visuals().weak_text_color();

    egui::ScrollArea::both().id_salt("hiker-data-scroll").show(ui, |ui| {
        egui::Grid::new("hiker-data-grid")
            .striped(true)
            .spacing(egui::vec2(18.0, 4.0))
            .show(ui, |ui| {
                for (col, ty) in table.columns.iter().zip(&types) {
                    ui.vertical(|ui| {
                        // Extend (don't wrap) so each column sizes to its header, not the
                        // reverse — wrapping made "quantitative" break across two lines.
                        ui.add(egui::Label::new(egui::RichText::new(&col.name).strong())
                            .wrap_mode(egui::TextWrapMode::Extend));
                        ui.add(egui::Label::new(egui::RichText::new(type_label(*ty)).weak())
                            .wrap_mode(egui::TextWrapMode::Extend));
                    });
                }
                ui.end_row();

                for r in 0..rows {
                    for (col, ty) in table.columns.iter().zip(&types) {
                        let raw = col.cells.get(r).map_or("", String::as_str);
                        match coerce(raw, *ty) {
                            Value::Missing => {
                                ui.colored_label(weak, "—");
                            }
                            value => {
                                ui.label(format_value(&value));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    });
}

/// The short human label for an inferred data type, shown under each column header.
const fn type_label(ty: DataType) -> &'static str {
    match ty {
        DataType::Quantitative => "quantitative",
        DataType::Temporal => "temporal",
        DataType::Ordinal => "ordinal",
        DataType::Nominal => "nominal",
    }
}

#[cfg(test)]
mod tests {
    use super::data_view;
    use crate::model::BuilderState;
    use hiker_charts_core::backend::Size;
    use hiker_charts_core::data::Table;
    use hiker_charts_core::dsl::ChartSpec;
    use hiker_charts_core::theme::Theme;

    fn state() -> BuilderState {
        let spec = ChartSpec::from_yaml("mark: table\n").unwrap();
        let table = Table::from_csv(b"day,revenue\n2021-01-01,100\n2021-02-01,oops\n").unwrap();
        BuilderState::new(spec, table, Theme::default(), Size { width: 320, height: 240 })
    }

    #[test]
    fn data_view_runs_a_frame_without_panicking() {
        let s = state();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| data_view(&s, ui));
        });
    }
}
