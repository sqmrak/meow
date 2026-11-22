// an 8 bit rgba image and the few operations icon and launch image sets need

#[derive(Debug, Clone)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    // straight (not premultiplied) rgba, row by row
    pub px: Vec<u8>,
}

pub fn parse_color(s: &str) -> Result<[u8; 4], String> {
    let h = s.strip_prefix('#').ok_or_else(|| format!("color {:?} must look like #rrggbb", s))?;
    let ok = h.chars().all(|c| c.is_ascii_hexdigit());
    let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).unwrap_or(0);
    match (ok, h.len()) {
        (true, 3) => Ok([v(0, 1) * 17, v(1, 1) * 17, v(2, 1) * 17, 255]),
        (true, 6) => Ok([v(0, 2), v(2, 2), v(4, 2), 255]),
        (true, 8) => Ok([v(0, 2), v(2, 2), v(4, 2), v(6, 2)]),
        _ => Err(format!("color {:?} must look like #rgb, #rrggbb or #rrggbbaa", s)),
    }
}

fn lanczos(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

// for every output position the first input index and the normalized weights, the way
// pillow does it: the kernel is stretched by the scale when shrinking
fn contributions(inp: usize, out: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = inp as f64 / out as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    (0..out)
        .map(|o| {
            let center = (o as f64 + 0.5) * scale;
            let min = ((center - support + 0.5).floor().max(0.0)) as usize;
            let max = ((center + support + 0.5).floor().min(inp as f64)) as usize;
            let mut w: Vec<f64> = (min..max).map(|x| lanczos((x as f64 - center + 0.5) / filterscale)).collect();
            let sum: f64 = w.iter().sum();
            if sum != 0.0 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (min, w.into_iter().map(|v| v as f32).collect())
        })
        .collect()
}

impl Image {
    pub fn new(w: usize, h: usize) -> Image {
        Image { w, h, px: vec![0; w * h * 4] }
    }

    pub fn solid(w: usize, h: usize, c: [u8; 4]) -> Image {
        let mut i = Image::new(w, h);
        for p in i.px.chunks_mut(4) {
            p.copy_from_slice(&c);
        }
        i
    }

    pub fn get(&self, x: usize, y: usize) -> [u8; 4] {
        let o = (y * self.w + x) * 4;
        [self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]]
    }

    pub fn set(&mut self, x: usize, y: usize, c: [u8; 4]) {
        let o = (y * self.w + x) * 4;
        self.px[o..o + 4].copy_from_slice(&c);
    }

    // pil convert("RGB") of an rgba image: the color stays, the alpha goes
    pub fn drop_alpha(&mut self) {
        self.px.chunks_mut(4).for_each(|p| p[3] = 255);
    }

    // lanczos resize on premultiplied color, so a transparent edge does not bleed
    pub fn resize(&self, nw: usize, nh: usize) -> Result<Image, String> {
        if nw == 0 || nh == 0 || nw > 16_384 || nh > 16_384 {
            return Err(format!("cannot resize to {}x{}", nw, nh));
        }
        let mut src = vec![0f32; self.w * self.h * 4];
        for (i, p) in self.px.chunks(4).enumerate() {
            let a = p[3] as f32 / 255.0;
            src[4 * i] = p[0] as f32 * a;
            src[4 * i + 1] = p[1] as f32 * a;
            src[4 * i + 2] = p[2] as f32 * a;
            src[4 * i + 3] = p[3] as f32;
        }
        let hc = contributions(self.w, nw);
        let mut mid = vec![0f32; nw * self.h * 4];
        for y in 0..self.h {
            for (x, (first, w)) in hc.iter().enumerate() {
                let mut acc = [0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let o = (y * self.w + first + k) * 4;
                    for c in 0..4 {
                        acc[c] += src[o + c] * wk;
                    }
                }
                mid[(y * nw + x) * 4..(y * nw + x) * 4 + 4].copy_from_slice(&acc);
            }
        }
        let vc = contributions(self.h, nh);
        let mut out = Image::new(nw, nh);
        for (y, (first, w)) in vc.iter().enumerate() {
            for x in 0..nw {
                let mut acc = [0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let o = ((first + k) * nw + x) * 4;
                    for c in 0..4 {
                        acc[c] += mid[o + c] * wk;
                    }
                }
                let a = acc[3].clamp(0.0, 255.0);
                let px = if a <= 0.0 {
                    [0, 0, 0, 0]
                } else {
                    let f = 255.0 / a;
                    [(acc[0] * f).round().clamp(0.0, 255.0) as u8, (acc[1] * f).round().clamp(0.0, 255.0) as u8, (acc[2] * f).round().clamp(0.0, 255.0) as u8, a.round() as u8]
                };
                out.set(x, y, px);
            }
        }
        Ok(out)
    }

    // pillow thumbnail: keeps the aspect ratio, never enlarges
    pub fn fit(&self, maxw: usize, maxh: usize) -> Result<Image, String> {
        let scale = (maxw as f64 / self.w as f64).min(maxh as f64 / self.h as f64);
        if scale >= 1.0 {
            return Ok(self.clone());
        }
        let nw = ((self.w as f64 * scale).round() as usize).max(1);
        let nh = ((self.h as f64 * scale).round() as usize).max(1);
        self.resize(nw, nh)
    }

    // multiplies alpha by the coverage of a rounded rectangle that fills the image
    pub fn round(&mut self, radius: f64) {
        let (w, h) = (self.w as f64, self.h as f64);
        let r = radius.clamp(0.0, w.min(h) / 2.0);
        const N: usize = 4;
        for y in 0..self.h {
            for x in 0..self.w {
                let corner_x = (x as f64) < r || (x as f64 + 1.0) > w - r;
                let corner_y = (y as f64) < r || (y as f64 + 1.0) > h - r;
                if !(corner_x && corner_y) {
                    continue;
                }
                let mut inside = 0;
                for sy in 0..N {
                    for sx in 0..N {
                        let px = x as f64 + (sx as f64 + 0.5) / N as f64;
                        let py = y as f64 + (sy as f64 + 0.5) / N as f64;
                        let cx = px.clamp(r, w - r);
                        let cy = py.clamp(r, h - r);
                        if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                            inside += 1;
                        }
                    }
                }
                let o = (y * self.w + x) * 4 + 3;
                self.px[o] = (self.px[o] as f64 * inside as f64 / (N * N) as f64).round() as u8;
            }
        }
    }

    // pil alpha_composite of other over self at an offset, other may hang over the edge
    pub fn over(&mut self, other: &Image, ox: i64, oy: i64) {
        for y in 0..other.h {
            for x in 0..other.w {
                let (dx, dy) = (x as i64 + ox, y as i64 + oy);
                if dx < 0 || dy < 0 || dx >= self.w as i64 || dy >= self.h as i64 {
                    continue;
                }
                let s = other.get(x, y);
                let d = self.get(dx as usize, dy as usize);
                let (sa, da) = (s[3] as f64 / 255.0, d[3] as f64 / 255.0);
                let oa = sa + da * (1.0 - sa);
                let px = if oa <= 0.0 {
                    [0, 0, 0, 0]
                } else {
                    let mix = |sc: u8, dc: u8| ((sc as f64 * sa + dc as f64 * da * (1.0 - sa)) / oa).round().clamp(0.0, 255.0) as u8;
                    [mix(s[0], d[0]), mix(s[1], d[1]), mix(s[2], d[2]), (oa * 255.0).round() as u8]
                };
                self.set(dx as usize, dy as usize, px);
            }
        }
    }

    // like over, then the alpha of self is put back: a veil that must not widen the shape
    pub fn over_keep_alpha(&mut self, other: &Image) {
        let alpha: Vec<u8> = self.px.chunks(4).map(|p| p[3]).collect();
        self.over(other, 0, 0);
        for (p, a) in self.px.chunks_mut(4).zip(alpha) {
            p[3] = a;
        }
    }

    // the box of pixels whose alpha is at least the threshold
    pub fn alpha_box(&self, threshold: u8) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.get(x, y)[3] >= threshold {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        (x0 != usize::MAX).then_some((x0, y0, x1, y1))
    }

    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Image {
        let mut out = Image::new(x1 - x0, y1 - y0);
        for y in y0..y1 {
            for x in x0..x1 {
                out.set(x - x0, y - y0, self.get(x, y));
            }
        }
        out
    }

    // centered on a new canvas, the offset rounds down like integer division
    pub fn on_canvas(&self, w: usize, h: usize, background: [u8; 4]) -> Image {
        let mut c = Image::solid(w, h, background);
        let ox = (w as i64 - self.w as i64) / 2;
        let oy = (h as i64 - self.h as i64) / 2;
        c.over(self, ox, oy);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_and_bad_ones_are_named() {
        assert_eq!(parse_color("#8455c1").unwrap(), [0x84, 0x55, 0xc1, 255]);
        assert_eq!(parse_color("#fff").unwrap(), [255, 255, 255, 255]);
        assert_eq!(parse_color("#00000080").unwrap(), [0, 0, 0, 128]);
        for bad in ["fff", "#ggg", "#12345", "", "#"] {
            assert!(parse_color(bad).is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn resize_keeps_flat_colors_and_averages_detail() {
        let flat = Image::solid(40, 40, [10, 200, 30, 255]).resize(7, 9).unwrap();
        assert!(flat.px.chunks(4).all(|p| p == [10, 200, 30, 255]));
        // a 1 pixel checker shrunk by 8 lands near the middle gray
        let mut checker = Image::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                checker.set(x, y, [v, v, v, 255]);
            }
        }
        let small = checker.resize(8, 8).unwrap();
        assert!(small.px.chunks(4).all(|p| (p[0] as i32 - 128).abs() < 12), "{:?}", &small.px[..8]);
        assert!(small.resize(0, 3).is_err());
        // upscaling a single pixel keeps its color
        assert_eq!(Image::solid(1, 1, [5, 6, 7, 255]).resize(4, 4).unwrap().get(2, 2), [5, 6, 7, 255]);
    }

    #[test]
    fn transparent_edges_do_not_bleed_color() {
        // opaque red on the left, fully transparent (black) on the right
        let mut img = Image::new(8, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set(x, y, [255, 0, 0, 255]);
            }
        }
        let small = img.resize(4, 2).unwrap();
        for x in 0..4 {
            let p = small.get(x, 0);
            if p[3] > 0 {
                assert_eq!((p[0], p[1], p[2]), (255, 0, 0), "x {} {:?}", x, p);
            }
        }
    }

    #[test]
    fn fit_never_enlarges_and_keeps_the_aspect() {
        let img = Image::solid(100, 50, [1, 2, 3, 255]);
        let f = img.fit(40, 40).unwrap();
        assert_eq!((f.w, f.h), (40, 20));
        assert_eq!(img.fit(500, 500).unwrap().w, 100);
    }

    #[test]
    fn rounded_corners_are_transparent_and_the_middle_is_not() {
        let mut img = Image::solid(100, 100, [9, 9, 9, 255]);
        img.round(18.0);
        assert_eq!(img.get(0, 0)[3], 0);
        assert_eq!(img.get(50, 50)[3], 255);
        assert_eq!(img.get(50, 0)[3], 255);
        // the edge of the arc is partly covered somewhere on the diagonal
        assert!((0..10).any(|k| {
            let a = img.get(k, k)[3];
            a > 0 && a < 255
        }));
        // no rounding leaves a square alone
        let mut sq = Image::solid(10, 10, [1, 1, 1, 255]);
        sq.round(0.0);
        assert!(sq.px.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn compositing_and_the_alpha_box() {
        let mut base = Image::solid(4, 4, [0, 0, 255, 255]);
        let veil = Image::solid(4, 4, [255, 255, 255, 128]);
        base.over(&veil, 0, 0);
        let p = base.get(1, 1);
        assert_eq!(p[3], 255);
        assert!(p[0] > 120 && p[0] < 135 && p[2] == 255, "{:?}", p);
        let mut clear = Image::solid(4, 4, [0, 0, 0, 0]);
        clear.over_keep_alpha(&veil);
        assert_eq!(clear.get(0, 0)[3], 0);
        let mut art = Image::new(10, 10);
        art.set(3, 4, [1, 1, 1, 255]);
        art.set(6, 5, [1, 1, 1, 31]);
        assert_eq!(art.alpha_box(32), Some((3, 4, 4, 5)));
        assert_eq!(art.alpha_box(1), Some((3, 4, 7, 6)));
        assert_eq!(Image::new(3, 3).alpha_box(1), None);
        let c = art.crop(3, 4, 4, 5).on_canvas(5, 5, [0, 0, 0, 0]);
        assert_eq!(c.get(2, 2), [1, 1, 1, 255]);
    }
}
