//! Bill of quantities as GAEB DA XML 3.2, exchange phase 83 (Angebotsaufforderung:
//! quantities without prices), for import into trade software such as PAPA.
//!
//! One Titel per storey, one position per element name with its net area or
//! volume. The long text carries the other figures so the estimator sees them
//! next to the price.

use crate::config::{format_cm, Unit};
use crate::model::{Building, Kind};
use crate::report::{de, totals};
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

pub fn write_x83(b: &Building, units: &BTreeMap<(Kind, String), Unit>, project: &str, date: &str, path: &Path) -> Result<()> {
    std::fs::write(path, x83(b, units, project, date))?;
    Ok(())
}

/// `date` as YYYY-MM-DD.
pub fn x83(b: &Building, units: &BTreeMap<(Kind, String), Unit>, project: &str, date: &str) -> String {
    let thickness = thicknesses(b);
    let project = esc(project);
    let mut x = String::new();
    let _ = write!(
        x,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<GAEB xmlns="http://www.gaeb.de/GAEB_DA_XML/DA83/3.2">
  <GAEBInfo>
    <Version>3.2</Version>
    <VersDate>2013-10</VersDate>
    <Date>{date}</Date>
    <ProgSystem>Aufmaß by bauzertifikate.de</ProgSystem>
  </GAEBInfo>
  <PrjInfo>
    <NamePrj>{project}</NamePrj>
    <LblPrj>{project}</LblPrj>
    <Cur>EUR</Cur>
    <CurLbl>Euro</CurLbl>
  </PrjInfo>
  <Award>
    <DP>83</DP>
    <AwardInfo>
      <Cur>EUR</Cur>
      <CurLbl>Euro</CurLbl>
    </AwardInfo>
    <BoQ ID="BoQ">
      <BoQInfo>
        <Name>Aufmaß</Name>
        <LblBoQ>{project}</LblBoQ>
        <Date>{date}</Date>
        <OutlCompl>AllTxt</OutlCompl>
        <BoQBkdn><Type>BoQLevel</Type><Length>2</Length><Num>Yes</Num></BoQBkdn>
        <BoQBkdn><Type>Item</Type><Length>4</Length><Num>Yes</Num></BoQBkdn>
      </BoQInfo>
      <BoQBody>
"#
    );
    for (si, s) in b.storeys.iter().enumerate() {
        let title = si + 1;
        let _ = write!(
            x,
            r#"        <BoQCtgy ID="T{title}" RNoPart="{title:02}">
          <LblTx><p><span>{}</span></p></LblTx>
          <BoQBody>
            <Itemlist>
"#,
            esc(&s.name)
        );
        for (ii, ((kind, name), t)) in totals([s]).iter().enumerate() {
            let pos = (ii + 1) * 10;
            let unit = units.get(&(*kind, name.clone())).copied().unwrap_or_default();
            let (qty, qu) = match unit {
                Unit::M2 => (t.net, "m2"),
                Unit::M3 => (t.volume, "m3"),
            };
            let mut detail = vec![format!("{} {name}, Geschoss {}", kind.label(), s.name)];
            if let Some(d) = thickness.get(&(*kind, name.clone())) {
                detail.push(format!("Dicke {d} cm"));
            }
            detail.push(format!("{} Bauteil(e)", t.count));
            detail.push(format!("Fläche netto {} m², Öffnungen {} m²", de(t.net, 2), de(t.openings, 2)));
            detail.push(format!("Volumen {} m³", de(t.volume, 2)));
            let detail: String = detail.iter().map(|l| format!("<p><span>{}</span></p>", esc(l))).collect();
            let _ = write!(
                x,
                r#"              <Item ID="T{title}P{pos}" RNoPart="{pos:04}">
                <Qty>{qty:.3}</Qty>
                <QU>{qu}</QU>
                <Description>
                  <CompleteText>
                    <DetailTxt><Text>{detail}</Text></DetailTxt>
                    <OutlineText><OutlTxt><TextOutlTxt><p><span>{}</span></p></TextOutlTxt></OutlTxt></OutlineText>
                  </CompleteText>
                </Description>
              </Item>
"#,
                esc(&outline(name))
            );
        }
        x.push_str("            </Itemlist>\n          </BoQBody>\n        </BoQCtgy>\n");
    }
    x.push_str("      </BoQBody>\n    </BoQ>\n  </Award>\n</GAEB>\n");
    x
}

/// Today's date (UTC) as YYYY-MM-DD.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day), after Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Thickness per element name, when all elements of that name share it.
fn thicknesses(b: &Building) -> BTreeMap<(Kind, String), String> {
    let mut m: BTreeMap<(Kind, String), Option<String>> = BTreeMap::new();
    for e in b.storeys.iter().flat_map(|s| &s.elements) {
        let d = format_cm(e.thickness);
        m.entry((e.kind, e.name.clone())).and_modify(|v| if v.as_ref() != Some(&d) { *v = None }).or_insert(Some(d));
    }
    m.into_iter().filter_map(|(k, v)| Some((k, v?))).collect()
}

/// The short text is limited to 70 characters in GAEB.
fn outline(name: &str) -> String {
    name.chars().take(70).collect()
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Element, Storey};

    fn element(kind: Kind, name: &str, net: f64, volume: f64) -> Element {
        let v = crate::geom::V3::default();
        Element {
            id: String::new(),
            kind,
            name: name.into(),
            layer: String::new(),
            exterior: None,
            thickness: 0.18,
            length: 0.0,
            height: 0.0,
            pitch_deg: None,
            bottom_z: 0.0,
            gross_area: net,
            openings_area: 0.0,
            net_area: net,
            volume,
            outline: vec![],
            holes: vec![],
            normal: v,
            axis: v,
            start: None,
            end: None,
            openings: vec![],
            faces: vec![],
        }
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_758), (2026, 11, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn titles_per_storey_and_units_from_config() {
        let b = Building {
            units: "m",
            storeys: vec![
                Storey { name: "EG".into(), file: "eg.dwg".into(), elements: vec![
                    element(Kind::Wall, "Außenwand <Holz> & Co", 20.0, 7.3),
                    element(Kind::Wall, "Außenwand <Holz> & Co", 5.5, 2.0),
                    element(Kind::Slab, "BSP-Decke", 80.0, 14.4),
                ] },
                Storey { name: "OG".into(), file: "og.dwg".into(), elements: vec![element(Kind::Slab, "BSP-Decke", 60.0, 10.8)] },
            ],
            skipped: vec![],
        };
        let units = BTreeMap::from([((Kind::Slab, "BSP-Decke".to_string()), Unit::M3)]);
        let x = x83(&b, &units, "Haus Müller", "2026-11-01");
        assert!(x.contains(r#"<BoQCtgy ID="T1" RNoPart="01">"#));
        assert!(x.contains(r#"<BoQCtgy ID="T2" RNoPart="02">"#));
        assert!(x.contains("<Qty>25.500</Qty>\n                <QU>m2</QU>"));
        assert!(x.contains("<Qty>14.400</Qty>\n                <QU>m3</QU>"));
        assert!(x.contains("Außenwand &lt;Holz&gt; &amp; Co"));
        assert!(x.contains("Dicke 18 cm"));
        assert_eq!(x.matches("<Item ").count(), 3);
    }
}
