//! Decode BRSTM → PCM, change playback speed, re-encode as BRSTM.
//!
//! By default uses FFmpeg's `atempo` filter (time-stretch, pitch preserved), similar in spirit to
//! Audacity "Change Tempo" / sliding stretch. Use `--simple-resample` for naive resampling
//! (faster/slower + pitch shift, like speeding up a tape).

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use brstm::encoder::encode_brstm;
use brstm::BrstmInformation;

fn interleave(channels: &[Vec<i16>]) -> Vec<i16> {
    let n = channels[0].len();
    let mut out = Vec::with_capacity(n * channels.len());
    for i in 0..n {
        for ch in channels {
            out.push(ch[i]);
        }
    }
    out
}

fn deinterleave(interleaved: &[i16], num_channels: usize) -> Vec<Vec<i16>> {
    let frames = interleaved.len() / num_channels;
    let mut out: Vec<Vec<i16>> = (0..num_channels).map(|_| Vec::with_capacity(frames)).collect();
    for f in 0..frames {
        for c in 0..num_channels {
            out[c].push(interleaved[f * num_channels + c]);
        }
    }
    out
}

fn slice_pcm(channels: &[Vec<i16>], start: usize, end: usize) -> Vec<Vec<i16>> {
    channels
        .iter()
        .map(|ch| ch[start..end].to_vec())
        .collect()
}

fn concat_pcm(a: &[Vec<i16>], b: &[Vec<i16>]) -> Vec<Vec<i16>> {
    assert_eq!(a.len(), b.len(), "channel count mismatch");
    let nch = a.len();
    let mut out = Vec::with_capacity(nch);
    for c in 0..nch {
        let mut v = a[c].clone();
        v.extend_from_slice(&b[c]);
        out.push(v);
    }
    out
}

/// (1) Last `preview_sec` seconds of the stream (before wrap), (2) first `preview_sec` from `loop_start`.
fn loop_preview_segments(
    channels: &[Vec<i16>],
    sample_rate: u32,
    loop_start: u32,
    preview_sec: u32,
) -> (Vec<Vec<i16>>, Vec<Vec<i16>>) {
    let n = channels[0].len();
    let span = (sample_rate.saturating_mul(preview_sec)) as usize;
    let seg1_start = n.saturating_sub(span);
    let seg1 = slice_pcm(channels, seg1_start, n);
    let lp = loop_start as usize;
    let seg2_end = (lp + span).min(n);
    let seg2 = slice_pcm(channels, lp, seg2_end);
    (seg1, seg2)
}

/// Standard PCM WAV (16-bit LE), for preview / external tools.
fn write_wav_pcm16(path: &Path, channels: &[Vec<i16>], sample_rate: u32) -> Result<()> {
    let num_channels = channels.len() as u16;
    let interleaved = interleave(channels);
    let data_size = interleaved.len() * 2;
    let riff_chunk_size = 36u32 + data_size as u32;

    let mut w = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    w.write_all(b"RIFF")?;
    w.write_all(&riff_chunk_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&num_channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    let byte_rate = sample_rate * num_channels as u32 * 2;
    w.write_all(&byte_rate.to_le_bytes())?;
    let block_align = num_channels * 2;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&(data_size as u32).to_le_bytes())?;
    for s in &interleaved {
        w.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

/// FFmpeg `atempo` accepts 0.5..=2.0 per stage; chain filters for values outside.
fn build_atempo_filter_chain(tempo: f64) -> String {
    assert!(tempo > 0.0, "tempo must be positive");
    let mut t = tempo;
    let mut stages = Vec::new();
    while t > 2.0 {
        stages.push("atempo=2.0".to_string());
        t /= 2.0;
    }
    while t < 0.5 {
        stages.push("atempo=0.5".to_string());
        t /= 0.5;
    }
    stages.push(format!("atempo={:.9}", t));
    stages.join(",")
}

fn apply_ffmpeg_atempo(
    interleaved: &[i16],
    sample_rate: u32,
    channels: u16,
    tempo: f64,
) -> Result<Vec<i16>> {
    let filter = build_atempo_filter_chain(tempo);
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "s16le",
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-i",
            "pipe:0",
            "-filter:a",
            &filter,
            "-f",
            "s16le",
            "-ac",
            &channels.to_string(),
            "-ar",
            &sample_rate.to_string(),
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(
            "failed to run `ffmpeg`. Install FFmpeg and ensure it is on PATH \
             (https://ffmpeg.org/download.html). Pitch-preserving mode requires FFmpeg.",
        )?;

    let mut stdin = child.stdin.take().unwrap();
    let to_write = interleaved.to_vec();
    let writer = std::thread::spawn(move || {
        for sample in &to_write {
            stdin.write_all(&sample.to_le_bytes())?;
        }
        Ok::<_, std::io::Error>(())
    });

    let mut stdout = child.stdout.take().unwrap();
    let mut output_bytes = Vec::new();
    stdout
        .read_to_end(&mut output_bytes)
        .context("read ffmpeg stdout")?;

    writer.join().expect("stdin writer panicked")?;

    let mut stderr = Vec::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_end(&mut stderr);
    }
    let status = child.wait().context("wait for ffmpeg")?;
    if !status.success() {
        let msg = String::from_utf8_lossy(&stderr);
        anyhow::bail!("ffmpeg failed (exit {}): {}", status, msg.trim());
    }

    if output_bytes.len() % 2 != 0 {
        anyhow::bail!("ffmpeg output length is not a multiple of 2 (s16le)");
    }
    let mut out = Vec::with_capacity(output_bytes.len() / 2);
    for chunk in output_bytes.chunks_exact(2) {
        out.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    let ch = channels as usize;
    if !out.is_empty() && out.len() % ch != 0 {
        anyhow::bail!(
            "ffmpeg output sample count {} is not divisible by channel count {}",
            out.len(),
            ch
        );
    }
    Ok(out)
}

fn resample_speed(samples: &[i16], speed: f64) -> Vec<i16> {
    assert!(speed > 0.0, "speed must be positive");
    let n = samples.len();
    let new_len = ((n as f64) / speed).floor() as usize;
    if new_len == 0 {
        return vec![0];
    }
    let mut out = Vec::with_capacity(new_len);
    for j in 0..new_len {
        let src_pos = (j as f64) * speed;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - (idx as f64);
        let s0 = *samples.get(idx).unwrap_or(&0) as f64;
        let s1 = *samples.get(idx.saturating_add(1)).unwrap_or(&0) as f64;
        let s = s0 + (s1 - s0) * frac;
        out.push(s.clamp(-32768.0, 32767.0) as i16);
    }
    out
}

/// Rejects mistaken CLI where a flag is used as a path (e.g. `--export-wav --loop-preview out.wav`).
fn expect_file_path(p: &str, role: &str) -> Result<()> {
    let t = p.trim();
    if t.is_empty() {
        anyhow::bail!("{role} path is empty");
    }
    if t.starts_with('-') {
        anyhow::bail!(
            "{role} was \"{p}\", which looks like a flag, not a file.\n\
             Put the .brstm path immediately after the subcommand.\n\
             \n\
             Examples:\n\
               brstm_speed_tool --loop-preview mysong.brstm\n\
               brstm_speed_tool --loop-preview-wav mysong.brstm loop_demo.wav\n\
               brstm_speed_tool --export-wav mysong.brstm out.wav\n\
             \n\
             Wrong order (tries to open a file literally named \"--loop-preview\"):\n\
               brstm_speed_tool --export-wav --loop-preview out.wav"
        );
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "Usage: brstm_speed_tool <input.brstm> <output.brstm> [speed_factor] [--simple-resample]"
    );
    eprintln!("  Windows: drag a .brstm onto brstm_speed_tool.exe to write stem_F.brstm at 1.15 (ffmpeg).");
    eprintln!("  speed_factor defaults to 1.15 (15% faster).");
    eprintln!("  Default: pitch-preserving time stretch via FFmpeg `atempo` (needs ffmpeg on PATH).");
    eprintln!("  --simple-resample: naive resampling (changes pitch like tape speed); no FFmpeg.");
    eprintln!();
    eprintln!("Other commands:");
    eprintln!("  brstm_speed_tool --loop-info <file.brstm>");
    eprintln!("    Print one JSON line: sample_rate, channels, loop_flag, loop_start, total_samples, …");
    eprintln!("  brstm_speed_tool --export-wav <in.brstm> <out.wav>");
    eprintln!("    Decode ADPCM to a standard WAV (for preview).");
    eprintln!("  brstm_speed_tool --loop-preview <file.brstm>");
    eprintln!("    Play: last 5s before stream end, then 5s from loop start (needs ffplay on PATH).");
    eprintln!("  brstm_speed_tool --loop-preview-wav <in.brstm> <out.wav>");
    eprintln!("    Write that transition to WAV only (use: ffplay -nodisp -autoexit out.wav).");
    eprintln!();
    eprintln!("Note: the .brstm path must come immediately after the flag (not reversed with .wav).");
}

fn cmd_loop_info(path: &str) -> Result<()> {
    expect_file_path(path, "BRSTM file")?;
    let orig = fs::read(path).with_context(|| format!("read {}", path))?;
    let mut cursor = Cursor::new(&orig);
    let parsed = BrstmInformation::from_reader(&mut cursor).context("parse BRSTM header")?;
    let i = &parsed.info;
    let ch = parsed.channel_count();
    println!(
        "{{\"sample_rate\":{},\"channels\":{},\"codec\":{},\"loop_flag\":{},\"loop_start\":{},\"total_samples\":{}}}",
        i.sample_rate,
        ch,
        i.codec,
        i.loop_flag,
        i.loop_start,
        i.total_samples
    );
    Ok(())
}

fn cmd_export_wav(input: &str, output: &str) -> Result<()> {
    expect_file_path(input, "Input BRSTM")?;
    expect_file_path(output, "Output WAV")?;
    let orig = fs::read(input).with_context(|| format!("read {}", input))?;
    let mut cursor = Cursor::new(&orig);
    let parsed = BrstmInformation::from_reader(&mut cursor).context("parse BRSTM header")?;
    if parsed.info.codec != 2 {
        anyhow::bail!(
            "only 4-bit ADPCM BRSTM files (codec 2) are supported; this file uses codec {}",
            parsed.info.codec
        );
    }
    let sample_rate = parsed.info.sample_rate as u32;
    let data = parsed
        .into_with_data(&mut cursor)
        .context("load BRSTM ADPCM/PCM data")?;
    let n_ch = data.info.channel_count() as usize;
    let channels: Vec<Vec<i16>> = (0..n_ch)
        .map(|ch| data.get_pcm(ch as u8))
        .collect();
    write_wav_pcm16(Path::new(output), &channels, sample_rate)
        .with_context(|| format!("write {}", output))?;
    eprintln!("Exported WAV: {} ({} ch, {} Hz)", output, n_ch, sample_rate);
    Ok(())
}

const LOOP_PREVIEW_SEC: u32 = 5;

fn decode_loop_preview_parts(input: &str) -> Result<(Vec<Vec<i16>>, Vec<Vec<i16>>, u32)> {
    let orig = fs::read(input).with_context(|| format!("read {}", input))?;
    let mut cursor = Cursor::new(&orig);
    let parsed = BrstmInformation::from_reader(&mut cursor).context("parse BRSTM header")?;
    if parsed.info.loop_flag == 0 {
        anyhow::bail!("this BRSTM has no loop; nothing to preview");
    }
    if parsed.info.codec != 2 {
        anyhow::bail!(
            "only 4-bit ADPCM BRSTM files (codec 2) are supported; this file uses codec {}",
            parsed.info.codec
        );
    }
    let sample_rate = parsed.info.sample_rate as u32;
    let loop_start = parsed.info.loop_start;
    let data = parsed
        .into_with_data(&mut cursor)
        .context("load BRSTM ADPCM/PCM data")?;
    let n_ch = data.info.channel_count() as usize;
    let channels: Vec<Vec<i16>> = (0..n_ch)
        .map(|ch| data.get_pcm(ch as u8))
        .collect();
    let (seg1, seg2) = loop_preview_segments(&channels, sample_rate, loop_start, LOOP_PREVIEW_SEC);
    Ok((seg1, seg2, sample_rate))
}

fn print_loop_jump_banner() {
    eprintln!();
    eprintln!("  +--------------------------------------------------------------+");
    eprintln!("  |  >> LOOP JUMP (stitch): now at loop start — same stream     |");
    eprintln!("  +--------------------------------------------------------------+");
    eprintln!();
}

fn cmd_loop_preview_wav(input: &str, output: &str) -> Result<()> {
    expect_file_path(input, "Input BRSTM")?;
    expect_file_path(output, "Output WAV")?;
    let (seg1, seg2, sample_rate) = decode_loop_preview_parts(input)?;
    let combined = concat_pcm(&seg1, &seg2);
    write_wav_pcm16(Path::new(output), &combined, sample_rate)
        .with_context(|| format!("write {}", output))?;
    eprintln!(
        "Wrote loop transition preview ({}s tail + {}s from loop start): {}",
        LOOP_PREVIEW_SEC, LOOP_PREVIEW_SEC, output
    );
    Ok(())
}

fn cmd_loop_preview_play(input: &str) -> Result<()> {
    expect_file_path(input, "Input BRSTM")?;
    let (seg1, seg2, sample_rate) = decode_loop_preview_parts(input)?;
    let combined = concat_pcm(&seg1, &seg2);
    let pid = std::process::id();
    let tmp = std::env::temp_dir().join(format!("brstm_lp_{}.wav", pid));
    let tmp_path = tmp
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid temp path"))?;
    write_wav_pcm16(Path::new(tmp_path), &combined, sample_rate).context("write temp wav")?;

    let sr = sample_rate as f64;
    let stitch_secs = seg1[0].len() as f64 / sr;
    let tail_secs = stitch_secs;
    let intro_secs = seg2[0].len() as f64 / sr;
    let total_secs = combined[0].len() as f64 / sr;

    eprintln!();
    eprintln!(
        "  Seamless playback: {:.2}s tail + {:.2}s from loop start = {:.2}s (no gap).",
        tail_secs, intro_secs, total_secs
    );
    eprintln!(
        "  A marker prints at the stitch (~{:.2}s) while audio stays continuous.",
        stitch_secs
    );

    let mut child = spawn_ffplay(Path::new(tmp_path)).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e
    })?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let marker = std::thread::spawn(move || {
        let step = 0.05_f64;
        let mut elapsed = 0.0_f64;
        while elapsed < stitch_secs {
            if stop_clone.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_secs_f64(step));
            elapsed += step;
        }
        if !stop_clone.load(Ordering::Relaxed) {
            print_loop_jump_banner();
            eprintln!(
                "  [STITCH ~{:.2}s]  End of tail | Start of loop (same WAV, no click from this tool)",
                stitch_secs
            );
        }
    });

    let status = child.wait().context("wait for ffplay")?;
    stop.store(true, Ordering::Relaxed);
    let _ = marker.join();

    let _ = fs::remove_file(&tmp);

    if !status.success() {
        anyhow::bail!("ffplay exited with status {}", status);
    }
    eprintln!("  Preview finished.");
    Ok(())
}

fn spawn_ffplay(wav: &Path) -> Result<std::process::Child> {
    let path = wav.to_str().ok_or_else(|| anyhow::anyhow!("invalid wav path"))?;
    for exe in ["ffplay", "ffplay.exe"] {
        if let Ok(child) = Command::new(exe)
            .args(["-nodisp", "-autoexit", "-loglevel", "quiet", path])
            .spawn()
        {
            return Ok(child);
        }
    }
    anyhow::bail!(
        "could not run ffplay (install FFmpeg and ensure ffplay is on PATH).\n\
         Try: brstm_speed_tool --loop-preview-wav \"...\" loop_demo.wav\n\
         then: ffplay -nodisp -autoexit loop_demo.wav"
    );
}

fn cmd_convert(input: &str, output: &str, speed: f64, simple_resample: bool) -> Result<()> {
    expect_file_path(input, "Input BRSTM")?;
    expect_file_path(output, "Output BRSTM")?;
    let orig = fs::read(input).with_context(|| format!("read {}", input))?;
    let mut cursor = Cursor::new(&orig);
    let parsed = BrstmInformation::from_reader(&mut cursor).context("parse BRSTM header")?;
    if parsed.info.codec != 2 {
        anyhow::bail!(
            "only 4-bit ADPCM BRSTM files (codec 2) are supported; this file uses codec {}",
            parsed.info.codec
        );
    }
    let sample_rate = parsed.info.sample_rate;
    let loop_flag = parsed.info.loop_flag != 0;
    let loop_start = if loop_flag {
        Some(parsed.info.loop_start)
    } else {
        None
    };

    let data = parsed
        .into_with_data(&mut cursor)
        .context("load BRSTM ADPCM/PCM data")?;

    let n_ch = data.info.channel_count() as usize;
    let mut channels: Vec<Vec<i16>> = (0..n_ch)
        .map(|ch| data.get_pcm(ch as u8))
        .collect();

    if simple_resample {
        for ch in &mut channels {
            *ch = resample_speed(ch, speed);
        }
    } else {
        let sr = sample_rate as u32;
        let interleaved = interleave(&channels);
        let out_i = apply_ffmpeg_atempo(&interleaved, sr, n_ch as u16, speed)?;
        channels = deinterleave(&out_i, n_ch);
    }

    let new_loop = loop_start.map(|lp| {
        let n = lp as f64 / speed;
        let nl = n.round() as u32;
        let max_lp = channels[0].len().saturating_sub(1) as u32;
        nl.min(max_lp)
    });

    let out_brstm = encode_brstm(&channels, sample_rate, new_loop)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let mut dest = Vec::new();
    out_brstm
        .write_brstm(&mut Cursor::new(&mut dest))
        .context("serialize BRSTM")?;
    fs::write(output, dest).with_context(|| format!("write {}", output))?;

    eprintln!(
        "Done: {} → {} ({} ch, {} Hz, speed={}{})",
        input,
        output,
        n_ch,
        sample_rate,
        speed,
        if simple_resample {
            ", mode=simple-resample (pitch shifts)"
        } else {
            ", mode=atempo (pitch preserved)"
        }
    );
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    // Windows drag-and-drop passes one path: write stem_F.brstm next to the source at 1.15.
    if args.len() == 2 {
        let input_path = args[1].trim();
        if !input_path.starts_with('-') {
            let path = Path::new(input_path);
            let is_brstm = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("brstm"))
                .unwrap_or(false);
            if is_brstm {
                if !path.is_file() {
                    anyhow::bail!("drag-and-drop: file not found: {}", input_path);
                }
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| anyhow::anyhow!("drag-and-drop: invalid file name"))?;
                let out_path = path.with_file_name(format!("{}_F.brstm", stem));
                let output = out_path.to_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "drag-and-drop: output path must be UTF-8 (try a path without special characters)"
                    )
                })?;
                eprintln!(
                    "Drag-and-drop: {} -> {} (speed 1.15, pitch preserved; needs ffmpeg on PATH)",
                    input_path, output
                );
                return cmd_convert(input_path, output, 1.15, false);
            }
        }
    }

    match args[1].as_str() {
        "--loop-info" => {
            if args.len() != 3 {
                anyhow::bail!("Usage: brstm_speed_tool --loop-info <file.brstm>");
            }
            cmd_loop_info(&args[2])?;
        }
        "--export-wav" => {
            if args.len() != 4 {
                anyhow::bail!("Usage: brstm_speed_tool --export-wav <in.brstm> <out.wav>");
            }
            cmd_export_wav(&args[2], &args[3])?;
        }
        "--loop-preview" => {
            if args.len() != 3 {
                anyhow::bail!("Usage: brstm_speed_tool --loop-preview <file.brstm>");
            }
            cmd_loop_preview_play(&args[2])?;
        }
        "--loop-preview-wav" => {
            if args.len() != 4 {
                anyhow::bail!("Usage: brstm_speed_tool --loop-preview-wav <in.brstm> <out.wav>");
            }
            cmd_loop_preview_wav(&args[2], &args[3])?;
        }
        _ => {
            if args.len() < 3 {
                print_usage();
                std::process::exit(1);
            }
            let input = &args[1];
            let output = &args[2];
            let mut speed: Option<f64> = None;
            let mut simple_resample = false;
            for a in args.iter().skip(3) {
                match a.as_str() {
                    "--simple-resample" => simple_resample = true,
                    s if s.starts_with('-') => {
                        anyhow::bail!("unknown option: {} (try --simple-resample)", s);
                    }
                    s => {
                        if speed.is_some() {
                            anyhow::bail!("unexpected extra argument: {}", s);
                        }
                        speed = Some(s.parse().context("speed_factor must be a number, e.g. 1.15")?);
                    }
                }
            }
            let speed = speed.unwrap_or(1.15);
            cmd_convert(input, output, speed, simple_resample)?;
        }
    }
    Ok(())
}
