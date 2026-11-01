//! Quantity list as Excel workbook and console summary.

use crate::config::format_cm;
use crate::model::{Building, Kind, OpeningKind, Storey};
use anyhow::Result;
use rust_xlsxwriter::{Format, Workbook, Worksheet};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
pub struct Sum {
    pub count: usize,
    pub net: f64,
    pub openings: f64,
    pub volume: f64,
}

impl Sum {
    fn add(&mut self, o: &Sum) {
        self.count += o.count;
        self.net += o.net;
        self.openings += o.openings;
        self.volume += o.volume;
    }
}

pub type Totals = BTreeMap<(Kind, String), Sum>;

/// Totals per (kind, name) over the given storeys.
pub fn totals<'a>(storeys: impl IntoIterator<Item = &'a Storey>) -> Totals {
    let mut m = Totals::new();
    for e in storeys.into_iter().flat_map(|s| &s.elements) {
        let t = m.entry((e.kind, e.name.clone())).or_default();
        t.count += 1;
        t.net += e.net_area;
        t.openings += e.openings_area;
        t.volume += e.volume;
    }
    m
}

/// Sum of all rows of one element type.
pub fn subtotal(rows: &Totals, kind: Kind) -> Sum {
    let mut s = Sum::default();
    rows.iter().filter(|((k, _), _)| *k == kind).for_each(|(_, t)| s.add(t));
    s
}

/// Sum of all rows. Only count and volume are meaningful: wall, slab and roof
/// areas do not add up to anything a price is based on.
pub fn grand_total(rows: &Totals) -> Sum {
    let mut s = Sum::default();
    rows.values().for_each(|t| s.add(t));
    s
}

/// German number format: 1234.5 -> "1.234,50"
pub fn de(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let (sign, digits) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped},{frac}") }
}

pub fn print_summary(b: &Building) {
    let rows = totals(&b.storeys);
    let line = |kind: &str, name: &str, t: &Sum| {
        println!(
            "{kind:<7} {name:<28} {:>6} {:>10} m² {:>10} m² {:>8} m³",
            t.count, de(t.net, 2), de(t.openings, 2), de(t.volume, 2)
        )
    };
    println!("\n{:<7} {:<28} {:>6} {:>13} {:>13} {:>11}", "Bauteil", "Bezeichnung", "Anzahl", "Fläche netto", "Öffnungen", "Volumen");
    for kind in Kind::ALL {
        let mut group = rows.iter().filter(|((k, _), _)| *k == kind).peekable();
        if group.peek().is_none() {
            continue;
        }
        for ((_, name), t) in group {
            line(kind.label(), name, t);
        }
        line("", &format!("Summe {}", kind.plural()), &subtotal(&rows, kind));
        println!();
    }
    let all = grand_total(&rows);
    println!("{:<7} {:<28} {:>6} {:>13} {:>13} {:>8} m³", "", "Gesamt", all.count, "", "", de(all.volume, 2));
    let openings: Vec<_> = b.storeys.iter().flat_map(|s| s.elements.iter().flat_map(|e| &e.openings)).collect();
    let count = |k: OpeningKind| openings.iter().filter(|o| o.kind == k).count();
    println!(
        "\nÖffnungen in allen Wandschichten: {} ({} Türen, {} Fenster, {} sonstige)",
        openings.len(),
        count(OpeningKind::Door),
        count(OpeningKind::Window),
        count(OpeningKind::Other)
    );
}

struct Formats {
    plain: Format,
    bold: Format,
    num: Format,
    bold_num: Format,
}

pub fn write_xlsx(b: &Building, path: &Path) -> Result<()> {
    let mut wb = Workbook::new();
    let f = Formats {
        plain: Format::new(),
        bold: Format::new().set_bold(),
        num: Format::new().set_num_format("#,##0.00"),
        bold_num: Format::new().set_bold().set_num_format("#,##0.00"),
    };
    let (bold, m2) = (&f.bold, &f.num);

    let ws = wb.add_worksheet().set_name("Zusammenfassung")?;
    let mut row = 0;
    ws.write_string_with_format(row, 0, "Summe je Bauteil", bold)?;
    row += 1;
    header(ws, row, &["Bauteil", "Bezeichnung", "Anzahl", "Fläche netto [m²]", "Öffnungen [m²]", "Volumen [m³]"], bold)?;
    row = summary_block(ws, row, &[], &totals(&b.storeys), "Gesamt", &f)?;
    row += 2;
    ws.write_string_with_format(row, 0, "Je Geschoss", bold)?;
    row += 1;
    header(ws, row, &["Geschoss", "Bauteil", "Bezeichnung", "Anzahl", "Fläche netto [m²]", "Öffnungen [m²]", "Volumen [m³]"], bold)?;
    for s in &b.storeys {
        row = summary_block(ws, row, &[&s.name], &totals([s]), &format!("Summe {}", s.name), &f)?;
    }
    ws.autofit();

    let ws = wb.add_worksheet().set_name("Bauteile")?;
    header(
        ws,
        0,
        &[
            "Geschoss", "ID", "Bauteil", "Bezeichnung", "Layer", "Außen/Innen", "Dicke [cm]", "Länge [m]", "Höhe [m]",
            "Neigung [°]", "UK [m]", "Fläche brutto [m²]", "Öffnungen [m²]", "Fläche netto [m²]", "Volumen [m³]", "Öffnungen",
        ],
        bold,
    )?;
    let mut row = 0;
    for s in &b.storeys {
        for e in &s.elements {
            row += 1;
            ws.write_string(row, 0, &s.name)?;
            ws.write_string(row, 1, &e.id)?;
            ws.write_string(row, 2, e.kind.label())?;
            ws.write_string(row, 3, &e.name)?;
            ws.write_string(row, 4, &e.layer)?;
            ws.write_string(row, 5, match e.exterior { Some(true) => "außen", Some(false) => "innen", None => "" })?;
            ws.write_string(row, 6, format_cm(e.thickness))?;
            ws.write_number_with_format(row, 7, e.length, m2)?;
            ws.write_number_with_format(row, 8, e.height, m2)?;
            if let Some(p) = e.pitch_deg {
                ws.write_number(row, 9, p)?;
            }
            ws.write_number_with_format(row, 10, e.bottom_z, m2)?;
            ws.write_number_with_format(row, 11, e.gross_area, m2)?;
            ws.write_number_with_format(row, 12, e.openings_area, m2)?;
            ws.write_number_with_format(row, 13, e.net_area, m2)?;
            ws.write_number_with_format(row, 14, e.volume, m2)?;
            ws.write_number(row, 15, e.openings.len() as f64)?;
        }
    }
    ws.autofilter(0, 0, row.max(1), 15)?;
    ws.set_freeze_panes(1, 0)?;
    ws.autofit();

    let ws = wb.add_worksheet().set_name("Öffnungen")?;
    header(ws, 0, &["Geschoss", "Wand", "ID", "Art", "Breite [m]", "Höhe [m]", "Brüstung [m]", "Fläche [m²]", "Abstand Wandanfang [m]"], bold)?;
    let mut row = 0;
    for s in &b.storeys {
        for e in &s.elements {
            for o in &e.openings {
                row += 1;
                ws.write_string(row, 0, &s.name)?;
                ws.write_string(row, 1, &e.id)?;
                ws.write_string(row, 2, &o.id)?;
                ws.write_string(row, 3, o.kind.label())?;
                ws.write_number_with_format(row, 4, o.width, m2)?;
                ws.write_number_with_format(row, 5, o.height, m2)?;
                ws.write_number_with_format(row, 6, o.sill, m2)?;
                ws.write_number_with_format(row, 7, o.area, m2)?;
                ws.write_number_with_format(row, 8, o.offset, m2)?;
            }
        }
    }
    ws.autofilter(0, 0, row.max(1), 8)?;
    ws.set_freeze_panes(1, 0)?;
    ws.autofit();

    let ws = wb.add_worksheet().set_name("Übersprungen")?;
    header(ws, 0, &["Datei", "Layer", "Grund", "Anzahl"], bold)?;
    for (i, sk) in b.skipped.iter().enumerate() {
        let r = i as u32 + 1;
        ws.write_string(r, 0, &sk.file)?;
        ws.write_string(r, 1, &sk.layer)?;
        ws.write_string(r, 2, &sk.reason)?;
        ws.write_number(r, 3, sk.count as f64)?;
    }
    ws.autofit();

    wb.save(path)?;
    Ok(())
}

/// Rows of `rows` after `row`, a bold subtotal per element type and a bold
/// total line labelled `total`. `lead` fills the first columns of every row
/// (the storey). Returns the last row written.
fn summary_block(ws: &mut Worksheet, mut row: u32, lead: &[&str], rows: &Totals, total: &str, f: &Formats) -> Result<u32> {
    let c = lead.len() as u16;
    let mut put = |row: u32, kind: &str, name: &str, t: &Sum, sub: bool, areas: bool| -> Result<()> {
        let (text, num) = if sub { (&f.bold, &f.bold_num) } else { (&f.plain, &f.num) };
        for (i, l) in lead.iter().enumerate() {
            ws.write_string_with_format(row, i as u16, *l, text)?;
        }
        ws.write_string_with_format(row, c, kind, text)?;
        ws.write_string_with_format(row, c + 1, name, text)?;
        ws.write_number_with_format(row, c + 2, t.count as f64, text)?;
        if areas {
            ws.write_number_with_format(row, c + 3, t.net, num)?;
            ws.write_number_with_format(row, c + 4, t.openings, num)?;
        }
        ws.write_number_with_format(row, c + 5, t.volume, num)?;
        Ok(())
    };
    for kind in Kind::ALL {
        let mut any = false;
        for ((_, name), t) in rows.iter().filter(|((k, _), _)| *k == kind) {
            row += 1;
            put(row, kind.label(), name, t, false, true)?;
            any = true;
        }
        if any {
            row += 1;
            put(row, "", &format!("Summe {}", kind.plural()), &subtotal(rows, kind), true, true)?;
        }
    }
    row += 1;
    put(row, "", total, &grand_total(rows), true, false)?;
    Ok(row)
}

fn header(ws: &mut Worksheet, row: u32, titles: &[&str], bold: &Format) -> Result<()> {
    for (c, t) in titles.iter().enumerate() {
        ws.write_string_with_format(row, c as u16, *t, bold)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn german_numbers() {
        assert_eq!(de(1234.5, 2), "1.234,50");
        assert_eq!(de(153.29, 2), "153,29");
        assert_eq!(de(-0.5, 1), "-0,5");
        assert_eq!(de(1_234_567.0, 0), "1.234.567");
    }
}
