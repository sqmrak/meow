use crate::image::{parse_color, Image};

// rasterizes the vector files the ui glyphs come as: svg with path, circle, ellipse,
// rect, line, polyline and polygon, fill and stroke with round, square or butt caps and
// round, bevel or miter joins, and android vector drawables with a path. anything else
// (transforms, gradients, clip paths, text) is an error, never a quietly wrong picture.
// coverage is exact horizontally and 8 samples vertically per pixel

const SUB: usize = 8;

type Pt = (f64, f64);

#[derive(Clone)]
struct Style {
    fill: Option<[u8; 3]>,
    stroke: Option<[u8; 3]>,
    stroke_width: f64,
    cap: String,
    join: String,
    miter_limit: f64,
    evenodd: bool,
    fill_opacity: f64,
    stroke_opacity: f64,
    opacity: f64,
}

impl Style {
    fn base() -> Style {
        Style { fill: Some([0, 0, 0]), stroke: None, stroke_width: 1.0, cap: "butt".into(), join: "miter".into(), miter_limit: 4.0, evenodd: false, fill_opacity: 1.0, stroke_opacity: 1.0, opacity: 1.0 }
    }

    fn apply(&mut self, name: &str, value: &str) -> Result<(), String> {
        let v = value.trim();
        let color = |v: &str| -> Result<Option<[u8; 3]>, String> {
            match v {
                "none" | "transparent" => Ok(None),
                "white" => Ok(Some([255, 255, 255])),
                "black" => Ok(Some([0, 0, 0])),
                "currentColor" => Ok(Some([0, 0, 0])),
                _ => parse_color(v).map(|c| Some([c[0], c[1], c[2]])),
            }
        };
        let num = |v: &str| v.trim_end_matches("px").parse::<f64>().map_err(|_| format!("bad number {:?} for {}", v, name));
        match name {
            "fill" => self.fill = color(v)?,
            "stroke" => self.stroke = color(v)?,
            "stroke-width" => self.stroke_width = num(v)?,
            "stroke-linecap" => self.cap = v.to_string(),
            "stroke-linejoin" => self.join = v.to_string(),
            "stroke-miterlimit" => self.miter_limit = num(v)?,
            "fill-rule" => self.evenodd = v == "evenodd",
            "fill-opacity" => self.fill_opacity = num(v)?,
            "stroke-opacity" => self.stroke_opacity = num(v)?,
            "opacity" => self.opacity = num(v)?,
            _ => {}
        }
        if !["butt", "round", "square"].contains(&self.cap.as_str()) || !["miter", "round", "bevel"].contains(&self.join.as_str()) {
            return Err(format!("unsupported stroke-linecap or stroke-linejoin ({}, {})", self.cap, self.join));
        }
        Ok(())
    }
}

struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    closing: bool,
    selfclose: bool,
}

fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn tags(text: &str) -> Result<Vec<Tag>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->").ok_or("unterminated comment")? + 3..];
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = &rest[rest.find('>').ok_or("unterminated declaration")? + 1..];
            continue;
        }
        let end = rest.find('>').ok_or("unterminated tag")?;
        let mut body = &rest[1..end];
        rest = &rest[end + 1..];
        let closing = body.starts_with('/');
        if closing {
            body = &body[1..];
        }
        let selfclose = body.ends_with('/');
        if selfclose {
            body = &body[..body.len() - 1];
        }
        let body = body.trim();
        let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
        let name = body[..name_end].to_string();
        let mut attrs = Vec::new();
        let mut a = body[name_end..].trim_start();
        while !a.is_empty() {
            let eq = a.find('=').ok_or_else(|| format!("attribute without a value in <{}>", name))?;
            let key = a[..eq].trim().to_string();
            let after = a[eq + 1..].trim_start();
            let q = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| format!("unquoted attribute {} in <{}>", key, name))?;
            let close = after[1..].find(q).ok_or("unterminated attribute value")?;
            attrs.push((key, decode_entities(&after[1..1 + close])));
            a = after[close + 2..].trim_start();
        }
        out.push(Tag { name, attrs, closing, selfclose });
    }
    Ok(out)
}

struct Path {
    // each subpath with a flag for closed
    subs: Vec<(Vec<Pt>, bool)>,
}

struct PathParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> PathParser<'a> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip();
        let start = self.i;
        if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
            self.i += 1;
        }
        let mut dot = false;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else if (c == b'e' || c == b'E') && self.i > start {
                self.i += 1;
                if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i]).unwrap_or("").parse().map_err(|_| format!("bad number in path data at {}", start))
    }

    // arc flags are single characters and may touch the next number
    fn flag(&mut self) -> Result<bool, String> {
        self.skip();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err("bad arc flag in path data".to_string()),
        }
    }

    fn more_numbers(&mut self) -> bool {
        self.skip();
        self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }
}

fn arc_points(p0: Pt, rx: f64, ry: f64, phi_deg: f64, large: bool, sweep: bool, p1: Pt, out: &mut Vec<Pt>) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        out.push(p1);
        return;
    }
    let phi = phi_deg.to_radians();
    let (cp, sp) = (phi.cos(), phi.sin());
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lambda = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cp * cxp - sp * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let th1 = ang(1.0, 0.0, (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut dth = ang((x1 - cxp) / rx, (y1 - cyp) / ry, (-x1 - cxp) / rx, (-y1 - cyp) / ry);
    if !sweep && dth > 0.0 {
        dth -= std::f64::consts::TAU;
    } else if sweep && dth < 0.0 {
        dth += std::f64::consts::TAU;
    }
    let steps = ((dth.abs() / 0.08).ceil() as usize).max(4);
    for k in 1..=steps {
        let t = th1 + dth * k as f64 / steps as f64;
        out.push((cp * rx * t.cos() - sp * ry * t.sin() + cx, sp * rx * t.cos() + cp * ry * t.sin() + cy));
    }
}

fn cubic(p0: Pt, c1: Pt, c2: Pt, p1: Pt, out: &mut Vec<Pt>) {
    const N: usize = 24;
    for k in 1..=N {
        let t = k as f64 / N as f64;
        let u = 1.0 - t;
        out.push((
            u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * p1.0,
            u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * p1.1,
        ));
    }
}

fn parse_path(d: &str) -> Result<Path, String> {
    let mut p = PathParser { s: d.as_bytes(), i: 0 };
    let mut subs: Vec<(Vec<Pt>, bool)> = Vec::new();
    let mut cur: Pt = (0.0, 0.0);
    let mut start: Pt = (0.0, 0.0);
    let mut last_ctl: Option<Pt> = None;
    let mut last_q: Option<Pt> = None;
    let mut cmd = b' ';
    loop {
        p.skip();
        if p.i >= p.s.len() {
            break;
        }
        let c = p.s[p.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            p.i += 1;
            if cmd == b'z' || cmd == b'Z' {
                if let Some(s) = subs.last_mut() {
                    s.1 = true;
                }
                cur = start;
                last_ctl = None;
                last_q = None;
                continue;
            }
        } else if !p.more_numbers() || cmd == b' ' {
            return Err(format!("unexpected character in path data at {}", p.i));
        }
        let rel = cmd.is_ascii_lowercase();
        let abs = |pt: Pt, cur: Pt| if rel { (cur.0 + pt.0, cur.1 + pt.1) } else { pt };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let pt = abs((p.number()?, p.number()?), cur);
                subs.push((vec![pt], false));
                cur = pt;
                start = pt;
                // further pairs after a moveto are linetos
                cmd = if rel { b'l' } else { b'L' };
                last_ctl = None;
                last_q = None;
            }
            b'L' | b'H' | b'V' => {
                let pt = match cmd.to_ascii_uppercase() {
                    b'H' => {
                        let x = p.number()?;
                        (if rel { cur.0 + x } else { x }, cur.1)
                    }
                    b'V' => {
                        let y = p.number()?;
                        (cur.0, if rel { cur.1 + y } else { y })
                    }
                    _ => abs((p.number()?, p.number()?), cur),
                };
                subs.last_mut().ok_or("path draws before a moveto")?.0.push(pt);
                cur = pt;
                last_ctl = None;
                last_q = None;
            }
            b'C' | b'S' => {
                let smooth = cmd.to_ascii_uppercase() == b'S';
                let c1 = if smooth { last_ctl.map_or(cur, |l| (2.0 * cur.0 - l.0, 2.0 * cur.1 - l.1)) } else { abs((p.number()?, p.number()?), cur) };
                let c2 = abs((p.number()?, p.number()?), cur);
                let pt = abs((p.number()?, p.number()?), cur);
                cubic(cur, c1, c2, pt, &mut subs.last_mut().ok_or("path draws before a moveto")?.0);
                cur = pt;
                last_ctl = Some(c2);
                last_q = None;
            }
            b'Q' | b'T' => {
                let smooth = cmd.to_ascii_uppercase() == b'T';
                let q = if smooth { last_q.map_or(cur, |l| (2.0 * cur.0 - l.0, 2.0 * cur.1 - l.1)) } else { abs((p.number()?, p.number()?), cur) };
                let pt = abs((p.number()?, p.number()?), cur);
                // a quadratic is a cubic with the control points moved two thirds toward q
                let c1 = (cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1));
                let c2 = (pt.0 + 2.0 / 3.0 * (q.0 - pt.0), pt.1 + 2.0 / 3.0 * (q.1 - pt.1));
                cubic(cur, c1, c2, pt, &mut subs.last_mut().ok_or("path draws before a moveto")?.0);
                cur = pt;
                last_q = Some(q);
                last_ctl = None;
            }
            b'A' => {
                let (rx, ry, rot) = (p.number()?, p.number()?, p.number()?);
                let (large, sweep) = (p.flag()?, p.flag()?);
                let pt = abs((p.number()?, p.number()?), cur);
                arc_points(cur, rx, ry, rot, large, sweep, pt, &mut subs.last_mut().ok_or("path draws before a moveto")?.0);
                cur = pt;
                last_ctl = None;
                last_q = None;
            }
            other => return Err(format!("unknown path command {}", other as char)),
        }
    }
    Ok(Path { subs })
}

// per subscanline lists of covered x intervals, merged into pixel coverage at the end
struct Layer {
    w: usize,
    h: usize,
    rows: Vec<Vec<(f64, f64)>>,
}

impl Layer {
    fn new(w: usize, h: usize) -> Layer {
        Layer { w, h, rows: vec![Vec::new(); h * SUB] }
    }

    fn fill(&mut self, polys: &[Vec<Pt>], evenodd: bool) {
        for (r, row) in self.rows.iter_mut().enumerate() {
            let y = (r as f64 + 0.5) / SUB as f64;
            let mut hits: Vec<(f64, i32)> = Vec::new();
            for poly in polys {
                let n = poly.len();
                for i in 0..n {
                    let (a, b) = (poly[i], poly[(i + 1) % n]);
                    if a.1 == b.1 {
                        continue;
                    }
                    let (lo, hi, dir) = if a.1 < b.1 { (a, b, 1) } else { (b, a, -1) };
                    if y >= lo.1 && y < hi.1 {
                        hits.push((lo.0 + (y - lo.1) * (hi.0 - lo.0) / (hi.1 - lo.1), dir));
                    }
                }
            }
            hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut wind = 0;
            for k in 0..hits.len() {
                let inside_before = if evenodd { wind % 2 != 0 } else { wind != 0 };
                wind += hits[k].1;
                let inside_after = if evenodd { wind % 2 != 0 } else { wind != 0 };
                if !inside_before && inside_after {
                    // the interval runs to the next crossing that leaves the shape
                    let mut w2 = wind;
                    for j in k + 1..hits.len() {
                        w2 += hits[j].1;
                        let still = if evenodd { w2 % 2 != 0 } else { w2 != 0 };
                        if !still {
                            row.push((hits[k].0, hits[j].0));
                            break;
                        }
                    }
                }
            }
        }
    }

    fn coverage(&self) -> Vec<f32> {
        let mut cov = vec![0f32; self.w * self.h];
        for (r, row) in self.rows.iter().enumerate() {
            let py = r / SUB;
            let mut v = row.clone();
            v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (a, b) in v {
                match merged.last_mut() {
                    Some(l) if a <= l.1 => l.1 = l.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            for (a, b) in merged {
                let (a, b) = (a.max(0.0), b.min(self.w as f64));
                if b <= a {
                    continue;
                }
                for x in a.floor() as usize..(b.ceil() as usize).min(self.w) {
                    let overlap = (b.min(x as f64 + 1.0) - a.max(x as f64)).max(0.0);
                    cov[py * self.w + x] += (overlap / SUB as f64) as f32;
                }
            }
        }
        cov.iter_mut().for_each(|c| *c = c.min(1.0));
        cov
    }
}

fn circle(c: Pt, r: f64) -> Vec<Pt> {
    let n = ((r * 12.0) as usize).clamp(16, 96);
    (0..n).map(|k| {
        let a = std::f64::consts::TAU * k as f64 / n as f64;
        (c.0 + r * a.cos(), c.1 + r * a.sin())
    }).collect()
}

// the outline of a stroke as a set of polygons whose union is the stroke
fn stroke_polys(subs: &[(Vec<Pt>, bool)], st: &Style, width: f64) -> Vec<Vec<Pt>> {
    let h = width / 2.0;
    let mut out = Vec::new();
    for (pts, closed) in subs {
        let mut p: Vec<Pt> = Vec::new();
        for &q in pts {
            if p.last().map_or(true, |l| (l.0 - q.0).abs() > 1e-9 || (l.1 - q.1).abs() > 1e-9) {
                p.push(q);
            }
        }
        if *closed && p.len() > 2 && (p[0].0 - p[p.len() - 1].0).abs() < 1e-9 && (p[0].1 - p[p.len() - 1].1).abs() < 1e-9 {
            p.pop();
        }
        if p.len() == 1 {
            // a zero length subpath draws a dot with round caps and nothing otherwise
            if st.cap == "round" {
                out.push(circle(p[0], h));
            }
            continue;
        }
        let n = p.len();
        let segs = if *closed { n } else { n - 1 };
        let dirs: Vec<Pt> = (0..segs)
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % n]);
                let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                ((b.0 - a.0) / l, (b.1 - a.1) / l)
            })
            .collect();
        for i in 0..segs {
            let (mut a, mut b) = (p[i], p[(i + 1) % n]);
            let d = dirs[i];
            if st.cap == "square" && !*closed {
                if i == 0 {
                    a = (a.0 - d.0 * h, a.1 - d.1 * h);
                }
                if i == segs - 1 {
                    b = (b.0 + d.0 * h, b.1 + d.1 * h);
                }
            }
            let nrm = (-d.1 * h, d.0 * h);
            out.push(vec![(a.0 + nrm.0, a.1 + nrm.1), (b.0 + nrm.0, b.1 + nrm.1), (b.0 - nrm.0, b.1 - nrm.1), (a.0 - nrm.0, a.1 - nrm.1)]);
        }
        let joints: Vec<usize> = if *closed { (0..n).collect() } else { (1..n - 1).collect() };
        for j in joints {
            let d1 = dirs[(j + segs - 1) % segs];
            let d2 = dirs[j % segs];
            match st.join.as_str() {
                "round" => out.push(circle(p[j], h)),
                _ => {
                    let cross = d1.0 * d2.1 - d1.1 * d2.0;
                    if cross.abs() < 1e-9 {
                        continue;
                    }
                    // the outer side of the turn
                    let s = if cross > 0.0 { -1.0 } else { 1.0 };
                    let n1 = (-d1.1 * h * s, d1.0 * h * s);
                    let n2 = (-d2.1 * h * s, d2.0 * h * s);
                    let (a, b) = ((p[j].0 + n1.0, p[j].1 + n1.1), (p[j].0 + n2.0, p[j].1 + n2.1));
                    let mut poly = vec![p[j], a];
                    if st.join == "miter" {
                        let cos_half = ((1.0 + d1.0 * d2.0 + d1.1 * d2.1) / 2.0).max(0.0).sqrt();
                        if cos_half > 1e-9 && 1.0 / cos_half <= st.miter_limit {
                            let m = (n1.0 + n2.0, n1.1 + n2.1);
                            let ml = (m.0 * m.0 + m.1 * m.1).sqrt();
                            let tip = h / cos_half;
                            poly.push((p[j].0 + m.0 / ml * tip, p[j].1 + m.1 / ml * tip));
                        }
                    }
                    poly.push(b);
                    out.push(poly);
                }
            }
        }
        if !*closed && st.cap == "round" {
            out.push(circle(p[0], h));
            out.push(circle(p[n - 1], h));
        }
    }
    out
}

fn shape_subs(tag: &Tag) -> Result<Vec<(Vec<Pt>, bool)>, String> {
    let f = |k: &str, d: f64| -> Result<f64, String> {
        match tag.attrs.iter().find(|(n, _)| n == k) {
            Some((_, v)) => v.trim_end_matches("px").parse().map_err(|_| format!("bad {} on <{}>", k, tag.name)),
            None => Ok(d),
        }
    };
    let attr = |k: &str| tag.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    match tag.name.as_str() {
        "path" => Ok(parse_path(attr("d").or_else(|| attr("android:pathData")).ok_or("path without d")?)?.subs),
        "circle" => {
            let (cx, cy, r) = (f("cx", 0.0)?, f("cy", 0.0)?, f("r", 0.0)?);
            Ok(vec![(circle((cx, cy), r), true)])
        }
        "ellipse" => {
            let (cx, cy, rx, ry) = (f("cx", 0.0)?, f("cy", 0.0)?, f("rx", 0.0)?, f("ry", 0.0)?);
            let pts = circle((0.0, 0.0), 1.0).into_iter().map(|(x, y)| (cx + x * rx, cy + y * ry)).collect();
            Ok(vec![(pts, true)])
        }
        "rect" => {
            let (x, y, w, h) = (f("x", 0.0)?, f("y", 0.0)?, f("width", 0.0)?, f("height", 0.0)?);
            let mut rx = f("rx", f("ry", 0.0)?)?;
            let mut ry = f("ry", rx)?;
            rx = rx.min(w / 2.0);
            ry = ry.min(h / 2.0);
            if rx <= 0.0 || ry <= 0.0 {
                return Ok(vec![(vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true)]);
            }
            let mut pts = Vec::new();
            let corner = |cx: f64, cy: f64, a0: f64, pts: &mut Vec<Pt>| {
                for k in 0..=8 {
                    let a = (a0 + 90.0 * k as f64 / 8.0).to_radians();
                    pts.push((cx + rx * a.cos(), cy + ry * a.sin()));
                }
            };
            corner(x + w - rx, y + ry, -90.0, &mut pts);
            corner(x + w - rx, y + h - ry, 0.0, &mut pts);
            corner(x + rx, y + h - ry, 90.0, &mut pts);
            corner(x + rx, y + ry, 180.0, &mut pts);
            Ok(vec![(pts, true)])
        }
        "line" => Ok(vec![(vec![(f("x1", 0.0)?, f("y1", 0.0)?), (f("x2", 0.0)?, f("y2", 0.0)?)], false)]),
        "polyline" | "polygon" => {
            let nums: Vec<f64> = attr("points").unwrap_or("").split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(|s| s.parse::<f64>().map_err(|_| format!("bad points on <{}>", tag.name))).collect::<Result<_, _>>()?;
            if nums.len() % 2 != 0 || nums.len() < 4 {
                return Err(format!("<{}> needs an even count of at least 4 numbers", tag.name));
            }
            Ok(vec![(nums.chunks(2).map(|c| (c[0], c[1])).collect(), tag.name == "polygon")])
        }
        other => Err(format!("unsupported element <{}>", other)),
    }
}

fn style_of(parent: &Style, tag: &Tag) -> Result<Style, String> {
    let mut st = parent.clone();
    for (k, v) in &tag.attrs {
        match k.as_str() {
            "transform" => return Err(format!("transform on <{}> is not supported", tag.name)),
            "style" => {
                for decl in v.split(';').filter(|d| d.contains(':')) {
                    let (n, val) = decl.split_once(':').unwrap();
                    st.apply(n.trim(), val)?;
                }
            }
            "android:fillColor" => st.apply("fill", v)?,
            "android:fillType" => st.apply("fill-rule", if v == "evenOdd" { "evenodd" } else { "nonzero" })?,
            "fill-rule" | "fill" | "stroke" | "stroke-width" | "stroke-linecap" | "stroke-linejoin" | "stroke-miterlimit" | "fill-opacity" | "stroke-opacity" | "opacity" => st.apply(k, v)?,
            _ => {}
        }
    }
    Ok(st)
}

pub fn render(text: &str, width: usize, height: usize, tint: Option<[u8; 3]>) -> Result<Image, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(format!("cannot render at {}x{}", width, height));
    }
    let all = tags(text)?;
    let root = all.iter().find(|t| !t.closing && (t.name == "svg" || t.name == "vector")).ok_or("no <svg> or <vector> element")?;
    let get = |t: &Tag, k: &str| t.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let (vw, vh) = if let Some(vb) = get(root, "viewBox") {
        let n: Vec<f64> = vb.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(|s| s.parse().unwrap_or(f64::NAN)).collect();
        if n.len() != 4 || n.iter().any(|v| v.is_nan()) || n[2] <= 0.0 || n[3] <= 0.0 || n[0] != 0.0 || n[1] != 0.0 {
            return Err(format!("viewBox {:?} must be 0 0 width height", vb));
        }
        (n[2], n[3])
    } else if let (Some(w), Some(h)) = (get(root, "android:viewportWidth"), get(root, "android:viewportHeight")) {
        (w.parse().map_err(|_| "bad viewportWidth")?, h.parse().map_err(|_| "bad viewportHeight")?)
    } else {
        return Err("the root element needs a viewBox".to_string());
    };
    let (sx, sy) = (width as f64 / vw, height as f64 / vh);
    let mean = (sx + sy) / 2.0;

    // premultiplied float accumulator
    let mut acc = vec![0f32; width * height * 4];
    let mut stack: Vec<Style> = vec![style_of(&Style::base(), root)?];
    let mut depth_names: Vec<String> = vec![root.name.clone()];
    let mut seen_root = false;
    for t in &all {
        if !seen_root {
            if std::ptr::eq(t, root) {
                seen_root = true;
                if root.selfclose {
                    break;
                }
            }
            continue;
        }
        if t.closing {
            if depth_names.pop().as_deref() != Some(t.name.as_str()) && t.name != "svg" && t.name != "vector" {
                return Err(format!("unbalanced </{}>", t.name));
            }
            stack.pop();
            if depth_names.is_empty() {
                break;
            }
            continue;
        }
        if ["title", "desc", "metadata"].contains(&t.name.as_str()) {
            if !t.selfclose {
                depth_names.push(t.name.clone());
                stack.push(stack.last().unwrap().clone());
            }
            continue;
        }
        let parent = stack.last().unwrap().clone();
        let mut st = style_of(&parent, t)?;
        if t.name == "g" {
            if !t.selfclose {
                depth_names.push(t.name.clone());
                stack.push(st);
            }
            continue;
        }
        let subs = shape_subs(t)?;
        if let Some(c) = tint {
            if st.fill.is_some() {
                st.fill = Some(c);
            }
            if st.stroke.is_some() {
                st.stroke = Some(c);
            }
        }
        let dev: Vec<(Vec<Pt>, bool)> = subs.iter().map(|(p, c)| (p.iter().map(|&(x, y)| (x * sx, y * sy)).collect(), *c)).collect();
        let layers: [(Option<[u8; 3]>, f64, bool); 2] = [(st.fill, st.fill_opacity, true), (st.stroke, st.stroke_opacity, false)];
        for (color, opacity, is_fill) in layers {
            let Some(color) = color else { continue };
            let mut layer = Layer::new(width, height);
            if is_fill {
                // open subpaths are filled as if closed, as svg does
                let polys: Vec<Vec<Pt>> = dev.iter().filter(|(p, _)| p.len() >= 3).map(|(p, _)| p.clone()).collect();
                layer.fill(&polys, st.evenodd);
            } else {
                for poly in stroke_polys(&dev, &st, st.stroke_width * mean) {
                    layer.fill(&[poly], false);
                }
            }
            let cov = layer.coverage();
            let a_mul = (opacity * st.opacity) as f32;
            for (i, c) in cov.iter().enumerate() {
                let a = c * a_mul;
                if a <= 0.0 {
                    continue;
                }
                let o = i * 4;
                let keep = 1.0 - a;
                for k in 0..3 {
                    acc[o + k] = color[k] as f32 / 255.0 * a + acc[o + k] * keep;
                }
                acc[o + 3] = a + acc[o + 3] * keep;
            }
        }
        if !t.selfclose {
            depth_names.push(t.name.clone());
            stack.push(st);
        }
    }
    let mut img = Image::new(width, height);
    for (i, p) in acc.chunks(4).enumerate() {
        if p[3] <= 0.0 {
            continue;
        }
        let px = [(p[0] / p[3] * 255.0).round().clamp(0.0, 255.0) as u8, (p[1] / p[3] * 255.0).round().clamp(0.0, 255.0) as u8, (p[2] / p[3] * 255.0).round().clamp(0.0, 255.0) as u8, (p[3] * 255.0).round().clamp(0.0, 255.0) as u8];
        img.set(i % width, i / width, px);
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(body: &str) -> String {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\">{}</svg>", body)
    }

    #[test]
    fn a_filled_rect_covers_exactly_its_pixels() {
        let img = render(&svg("<rect x=\"4\" y=\"4\" width=\"8\" height=\"8\" fill=\"#ff0000\"/>"), 24, 24, None).unwrap();
        assert_eq!(img.get(5, 5), [255, 0, 0, 255]);
        assert_eq!(img.get(3, 5)[3], 0);
        assert_eq!(img.get(12, 5)[3], 0);
        // half a pixel of coverage at a fractional edge
        let half = render(&svg("<rect x=\"4.5\" y=\"4\" width=\"8\" height=\"8\"/>"), 24, 24, None).unwrap();
        assert!((half.get(4, 6)[3] as i32 - 128).abs() <= 2, "{}", half.get(4, 6)[3]);
    }

    #[test]
    fn scaling_the_output_scales_the_picture() {
        let big = render(&svg("<circle cx=\"12\" cy=\"12\" r=\"6\"/>"), 72, 72, None).unwrap();
        assert_eq!(big.get(36, 36)[3], 255);
        assert_eq!(big.get(36, 4)[3], 0);
        // area of a disc of radius 18 px is about 1018 px
        let area: f64 = big.px.chunks(4).map(|p| p[3] as f64 / 255.0).sum();
        assert!((area - 1017.9).abs() < 12.0, "{}", area);
    }

    #[test]
    fn strokes_use_the_width_caps_and_joins() {
        let line = render(&svg("<path d=\"M4 12h16\" fill=\"none\" stroke=\"#fff\" stroke-width=\"4\" stroke-linecap=\"round\"/>"), 24, 24, None).unwrap();
        assert_eq!(line.get(12, 12), [255, 255, 255, 255]);
        assert_eq!(line.get(12, 9)[3], 0);
        // the round cap reaches half a width past the end point
        assert!(line.get(2, 12)[3] > 100);
        assert_eq!(render(&svg("<path d=\"M4 12h16\" fill=\"none\" stroke=\"#fff\" stroke-width=\"4\"/>"), 24, 24, None).unwrap().get(2, 12)[3], 0);
        // a miter join fills the outer corner, a bevel does not
        let corner = |join: &str| render(&svg(&format!("<path d=\"M4 20V8h12\" fill=\"none\" stroke=\"#000\" stroke-width=\"4\" stroke-linejoin=\"{}\"/>", join)), 24, 24, None).unwrap();
        assert_eq!(corner("miter").get(2, 6)[3], 255);
        assert!(corner("bevel").get(2, 6)[3] < 128);
    }

    #[test]
    fn inherited_style_tint_and_even_odd() {
        let g = render("<svg viewBox=\"0 0 24 24\"><g fill=\"#00ff00\"><rect width=\"24\" height=\"24\"/></g></svg>", 24, 24, None).unwrap();
        assert_eq!(g.get(3, 3), [0, 255, 0, 255]);
        let tinted = render("<svg viewBox=\"0 0 24 24\" stroke=\"#fff\" fill=\"none\"><path d=\"M2 12h20\" stroke-width=\"2\"/></svg>", 24, 24, Some([10, 20, 30])).unwrap();
        assert_eq!(tinted.get(12, 12), [10, 20, 30, 255]);
        // two nested squares: even-odd leaves a hole, non-zero (same winding) does not
        let d = "M2 2h20v20H2z M8 8h8v8H8z";
        let eo = render(&format!("<svg viewBox=\"0 0 24 24\"><path d=\"{}\" fill-rule=\"evenodd\"/></svg>", d), 24, 24, None).unwrap();
        assert_eq!(eo.get(12, 12)[3], 0);
        assert_eq!(eo.get(4, 12)[3], 255);
        let nz = render(&format!("<svg viewBox=\"0 0 24 24\"><path d=\"{}\"/></svg>", d), 24, 24, None).unwrap();
        assert_eq!(nz.get(12, 12)[3], 255);
    }

    #[test]
    fn android_vectors_and_curves_and_arcs() {
        let xml = "<?xml version=\"1.0\"?><vector xmlns:android=\"http://schemas.android.com/apk/res/android\" android:width=\"24dp\" android:height=\"24dp\" android:viewportWidth=\"24\" android:viewportHeight=\"24\"><path android:pathData=\"M10,17l5,-5 -5,-5v10z\" android:fillColor=\"#FFFFFF\"/></vector>";
        let img = render(xml, 24, 24, None).unwrap();
        assert_eq!(img.get(11, 12), [255, 255, 255, 255]);
        assert_eq!(img.get(20, 12)[3], 0);
        // a circle made of two arcs, flags written without separators as icon tools do
        let circle = render(&svg("<path d=\"M12 4a8 8 0 1 0 0 16a8 8 0 1 0 0-16z\"/>"), 24, 24, None).unwrap();
        assert_eq!(circle.get(12, 12)[3], 255);
        assert_eq!(circle.get(2, 2)[3], 0);
        let tight = render(&svg("<path d=\"M12 4a8 8 0 100 16a8 8 0 100-16z\"/>"), 24, 24, None).unwrap();
        assert_eq!(tight.px, circle.px);
        // quadratic and smooth cubic commands stay inside the box
        let curves = render(&svg("<path d=\"M2 20Q12 0 22 20T12 12C4 4 20 4 12 12S2 2 2 20z\"/>"), 24, 24, None).unwrap();
        assert!(curves.px.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn unsupported_input_is_an_error() {
        for bad in [
            svg("<g transform=\"scale(2)\"><rect width=\"1\" height=\"1\"/></g>"),
            svg("<text>hi</text>"),
            svg("<path d=\"M1 1 X 2 2\"/>"),
            svg("<path d=\"L1 1\"/>"),
            svg("<rect width=\"1\" height=\"1\" stroke=\"#000\" stroke-linecap=\"weird\"/>"),
            "<svg width=\"24\" height=\"24\"><rect/></svg>".to_string(),
            "<svg viewBox=\"1 1 24 24\"></svg>".to_string(),
            "plain text".to_string(),
        ] {
            assert!(render(&bad, 24, 24, None).is_err(), "{}", bad);
        }
        assert!(render(&svg(""), 0, 24, None).is_err());
    }
}
