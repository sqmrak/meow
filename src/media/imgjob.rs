use crate::image::{parse_color, Image};
use std::fs;
use std::path::Path;

// `meow img`: a list of image jobs on one command line, run in order. every output is
// written next to its place and renamed over it, so a failed run leaves the old file

#[derive(Debug, Default, Clone)]
pub struct Job {
    pub svg: bool,
    pub from: Option<String>,
    pub to: String,
    pub size: Option<(usize, usize)>,
    pub fit: Option<(usize, usize)>,
    pub crop_alpha: Option<u8>,
    pub drop_alpha: bool,
    pub round: Option<f64>,
    pub overlay: Option<String>,
    pub overlay_keep_alpha: bool,
    pub canvas: Option<(usize, usize)>,
    pub background: Option<String>,
    pub solid: Option<(usize, usize)>,
    pub paste: Option<String>,
    pub paste_side: Option<usize>,
    pub paste_round: Option<f64>,
    pub color: Option<String>,
}

fn dims(v: &str) -> Result<(usize, usize), String> {
    let (a, b) = v.split_once('x').ok_or_else(|| format!("{:?} must look like 24x24", v))?;
    Ok((a.parse().map_err(|_| format!("bad width in {:?}", v))?, b.parse().map_err(|_| format!("bad height in {:?}", v))?))
}

pub fn parse_args(args: &[String]) -> Result<Vec<Job>, String> {
    let mut jobs: Vec<Job> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("img: {} needs a value", flag))
        };
        match flag {
            "--job" => jobs.push(Job::default()),
            "--svg" => jobs.push(Job { svg: true, ..Default::default() }),
            other => {
                let job = jobs.last_mut().ok_or_else(|| format!("img: {} comes before --job or --svg", other))?;
                match other {
                    "--from" => job.from = Some(value(&mut i)?),
                    "--to" => job.to = value(&mut i)?,
                    "--size" => job.size = Some(dims(&value(&mut i)?)?),
                    "--fit" => job.fit = Some(dims(&value(&mut i)?)?),
                    "--crop-alpha" => job.crop_alpha = Some(value(&mut i)?.parse().map_err(|_| "img: --crop-alpha needs 0 to 255".to_string())?),
                    "--drop-alpha" => job.drop_alpha = true,
                    "--round" => job.round = Some(value(&mut i)?.parse().map_err(|_| "img: --round needs a number".to_string())?),
                    "--overlay" => job.overlay = Some(value(&mut i)?),
                    "--overlay-keep-alpha" => job.overlay_keep_alpha = true,
                    "--canvas" => job.canvas = Some(dims(&value(&mut i)?)?),
                    "--background" => job.background = Some(value(&mut i)?),
                    "--solid" => job.solid = Some(dims(&value(&mut i)?)?),
                    "--paste" => job.paste = Some(value(&mut i)?),
                    "--paste-side" => job.paste_side = Some(value(&mut i)?.parse().map_err(|_| "img: --paste-side needs a number".to_string())?),
                    "--paste-round" => job.paste_round = Some(value(&mut i)?.parse().map_err(|_| "img: --paste-round needs a number".to_string())?),
                    "--color" => job.color = Some(value(&mut i)?),
                    _ => return Err(format!("img: unknown argument {}", other)),
                }
            }
        }
        i += 1;
    }
    if jobs.is_empty() {
        return Err("img: no jobs".to_string());
    }
    for j in &jobs {
        if j.to.is_empty() {
            return Err("img: every job needs --to".to_string());
        }
        if j.from.as_deref() == Some(j.to.as_str()) {
            return Err(format!("img: {} would overwrite its own source", j.to));
        }
    }
    Ok(jobs)
}

fn load(path: &str) -> Result<Image, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    crate::png::decode_png(&bytes).map_err(|e| format!("{}: {}", path, e))
}

fn run_job(j: &Job) -> Result<Image, String> {
    if j.svg {
        let from = j.from.as_ref().ok_or("img: an svg job needs --from")?;
        let (w, h) = j.size.ok_or("img: an svg job needs --size")?;
        let tint = j.color.as_deref().map(parse_color).transpose()?.map(|c| [c[0], c[1], c[2]]);
        let text = fs::read_to_string(from).map_err(|e| format!("{}: {}", from, e))?;
        return crate::svg::render(&text, w, h, tint).map_err(|e| format!("{}: {}", from, e));
    }
    let background = j.background.as_deref().map(parse_color).transpose()?;
    // a launch image: a solid canvas with the icon in the middle
    if let Some((w, h)) = j.solid {
        let mut canvas = Image::solid(w, h, background.unwrap_or([0, 0, 0, 255]));
        if let Some(p) = &j.paste {
            let mut icon = load(p)?;
            if let Some(r) = j.paste_round {
                icon.round(r * icon.w as f64);
            }
            let side = j.paste_side.ok_or("img: --paste needs --paste-side")?;
            let art = icon.resize(side, side)?;
            canvas.over(&art, (w as i64 - side as i64) / 2, (h as i64 - side as i64) / 2);
        }
        return Ok(canvas);
    }
    let from = j.from.as_ref().ok_or("img: a job needs --from or --solid")?;
    let mut img = load(from)?;
    if let Some(t) = j.crop_alpha {
        let (x0, y0, x1, y1) = img.alpha_box(t).ok_or_else(|| format!("{}: nothing is opaque enough to crop to", from))?;
        img = img.crop(x0, y0, x1, y1);
    }
    if j.drop_alpha {
        img.drop_alpha();
    }
    if let Some(r) = j.round {
        img.round(r * img.w as f64);
    }
    if let Some((w, h)) = j.size {
        img = img.resize(w, h)?;
    } else if let Some((w, h)) = j.fit {
        img = img.fit(w, h)?;
    }
    if let Some(o) = &j.overlay {
        let veil = load(o)?.resize(img.w, img.h)?;
        if j.overlay_keep_alpha {
            img.over_keep_alpha(&veil);
        } else {
            img.over(&veil, 0, 0);
        }
    }
    if let Some((w, h)) = j.canvas {
        img = img.on_canvas(w, h, background.unwrap_or([0, 0, 0, 0]));
    }
    Ok(img)
}

pub fn run(jobs: &[Job]) -> Result<usize, String> {
    for j in jobs {
        let img = run_job(j)?;
        let bytes = crate::png::encode_png(&img)?;
        let to = Path::new(&j.to);
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        let tmp = format!("{}.tmp", j.to);
        fs::write(&tmp, bytes).map_err(|e| format!("{}: {}", tmp, e))?;
        fs::rename(&tmp, to).map_err(|e| format!("{}: {}", j.to, e))?;
    }
    Ok(jobs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("meow-img-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn master(d: &Path) -> String {
        let mut img = Image::solid(200, 200, [200, 50, 50, 255]);
        for y in 80..120 {
            for x in 80..120 {
                img.set(x, y, [255, 255, 255, 255]);
            }
        }
        let p = d.join("master.png");
        fs::write(&p, crate::png::encode_png(&img).unwrap()).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn icons_launch_images_glyphs_and_sprites_come_out_right() {
        let d = tmp("all");
        let m = master(&d);
        let out = |n: &str| d.join(n).to_string_lossy().into_owned();
        let veil = d.join("veil.png");
        fs::write(&veil, crate::png::encode_png(&Image::solid(10, 10, [255, 255, 255, 60])).unwrap()).unwrap();
        let svg = d.join("g.svg");
        fs::write(&svg, "<svg viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"#fff\" stroke-width=\"2\"><path d=\"M4 12h16\"/></svg>").unwrap();
        // a mascot: opaque white square in a transparent frame, with one faint pixel that must not widen the crop
        let mut art = Image::new(100, 100);
        for y in 30..50 {
            for x in 30..50 {
                art.set(x, y, [255, 255, 255, 255]);
            }
        }
        art.set(90, 90, [255, 255, 255, 10]);
        fs::write(d.join("art.png"), crate::png::encode_png(&art).unwrap()).unwrap();
        let art_path = d.join("art.png").to_string_lossy().into_owned();
        let mut a = args(&["--job", "--from", &m, "--to", &out("plain.png"), "--size", "57x57", "--drop-alpha"]);
        a.extend(args(&["--job", "--from", &m, "--to", &out("deb.png"), "--size", "57x57", "--round", "0.18", "--overlay", veil.to_str().unwrap(), "--overlay-keep-alpha"]));
        a.extend(args(&["--job", "--solid", "320x480", "--background", "#8455c1", "--paste", &m, "--paste-side", "96", "--paste-round", "0.18", "--to", &out("Default.png")]));
        a.extend(args(&["--job", "--from", &art_path, "--to", &out("sprite.png"), "--crop-alpha", "32", "--fit", "24x24", "--canvas", "24x24"]));
        a.extend(args(&["--svg", "--from", svg.to_str().unwrap(), "--to", &out("glyph@2x.png"), "--size", "48x48", "--color", "#ff8000"]));
        let jobs = parse_args(&a).unwrap();
        assert_eq!(run(&jobs).unwrap(), 5);
        let read = |n: &str| crate::png::decode_png(&fs::read(d.join(n)).unwrap()).unwrap();
        let plain = read("plain.png");
        assert_eq!((plain.w, plain.h, plain.get(28, 28)), (57, 57, [255, 255, 255, 255]));
        // the deb icon keeps rounded corners and gets the veil on top
        let deb = read("deb.png");
        assert_eq!(deb.get(0, 0)[3], 0);
        let c = deb.get(5, 28);
        assert!(c[3] == 255 && c[1] > 50, "{:?}", c);
        let launch = read("Default.png");
        assert_eq!((launch.w, launch.h, launch.get(2, 2)), (320, 480, [0x84, 0x55, 0xc1, 255]));
        assert_eq!(launch.get(160, 240), [255, 255, 255, 255]);
        // the sprite is the cropped white square, centered
        let sprite = read("sprite.png");
        assert_eq!((sprite.w, sprite.h), (24, 24));
        assert_eq!(sprite.get(12, 12), [255, 255, 255, 255]);
        assert_eq!(sprite.get(0, 0)[3], 0);
        let glyph = read("glyph@2x.png");
        assert_eq!((glyph.w, glyph.get(24, 24)), (48, [255, 128, 0, 255]));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_failed_job_keeps_the_old_file_and_mistakes_are_named() {
        let d = tmp("fail");
        let target = d.join("keep.png");
        fs::write(&target, "old contents").unwrap();
        let bad = parse_args(&args(&["--job", "--from", d.join("missing.png").to_str().unwrap(), "--to", target.to_str().unwrap(), "--size", "8x8"])).unwrap();
        assert!(run(&bad).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old contents");
        assert!(parse_args(&[]).unwrap_err().contains("no jobs"));
        assert!(parse_args(&args(&["--from", "a"])).unwrap_err().contains("before --job"));
        assert!(parse_args(&args(&["--job", "--from", "a.png", "--to", "a.png"])).unwrap_err().contains("overwrite"));
        assert!(parse_args(&args(&["--job", "--to", "x", "--size", "big"])).is_err());
        assert!(parse_args(&args(&["--job", "--to", "x", "--nope"])).is_err());
        // a jpeg is refused with advice, not decoded wrong
        fs::write(d.join("p.jpg"), [0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0, 0, 0]).unwrap();
        let jpg = parse_args(&args(&["--job", "--from", d.join("p.jpg").to_str().unwrap(), "--to", d.join("o.png").to_str().unwrap(), "--size", "8x8"])).unwrap();
        assert!(run(&jpg).unwrap_err().contains("convert the source to png"));
        fs::remove_dir_all(&d).unwrap();
    }
}
