# brstm_speed_tool
A tool for speeding up .brstm files. Inspired by Atlas' Final Lap Stream Maker, but with the difference that this speeds up the input song file without increasing its pitch, making it perfect for creating sped-up versions of New Super Mario Bros. Wii custom music. It supports brstm files so it can be used for other games too, but it was created for NSMBW.
# REQUIREMENTS (read before downloading)
- [FFmpeg](https://ffmpeg.org/download.html) (must be added to PATH);
- ffplay (for playing the loop points in the command line).
# Usage
This tool comes with a few commands:<br/>
(if these don't work, try adding `.\` or the full path to brstm_speed_tool.exe before `brstm_speed_tool.exe`)<br/>
`brstm_speed_tool.exe in.brstm out.brstm 1.15`<br/>
Where `in.brstm` is the name of the input file, and `out.brstm` is the name of the output file. `1.15` is the speed-up value, which, in the case of 1.15, speeds up the song by 15%;<br/>
`brstm_speed_tool.exe --loop-preview file.brstm`<br/>
This command plays the last 5 seconds of the song before the end loop and 5 seconds after the start loop. Replace `file.brstm` with the name of the file you want to listen to;<br/>
`brstm_speed_tool.exe in.brstm out.brstm --simple-resample 1.15`<br/>
This command does the same as the first one listed here, with the difference in the `--simple-resample` flag, that simply speeds up the song, also increasing the pitch, which is not ideal if you're looking to make a sped-up version for New Super Mario Bros. Wii;<br/>
`brstm_speed_tool --loop-info <file.brstm>`<br/>
Print one JSON line: sample_rate, channels, loop_flag, loop_start, total_samples, …;<br/>
`brstm_speed_tool --export-wav <in.brstm> <out.wav>`<br/>
Decode ADPCM to a standard WAV (for preview);<br/>
`brstm_speed_tool --loop-preview-wav <in.brstm> <out.wav>`<br/>
Write that transition to WAV only.<br/>

### NOTE: you can drag and drop the file you want to speed up onto the brstm_speed_tool.exe file and it'll automatically create a 15% sped up version with no pitch change, akin to the first command listed. This feature was added in to make it easier to use without the need to use a command line<br/>

# Building
If you want to build the project from zero, you can start by downloading the source code (Code -> Download ZIP), extract it to a folder, then you need to make sure you have the following package installed:<br/>
- [Rust](https://rust-lang.org/tools/install/)<br/>
1) Extract the contents of the zip to a folder of your choice;<br/>
2) Open a terminal in the newly extracted folder and paste the command `cd source`;<br/>
3) Now paste the command `cargo build --release`<br/>
The binary will be at `source/target/release/brstm_speed_tool.exe`    

# End Note
I created this program using [Cursor](cursor.com). Having never heard of vibecoding before, I wanted to give it a try and, while I am satisfied with the end result here, I disliked how boring the creation process was
### <ins> Should I ever make another program, it'll be 100% coded by me from scratch. <ins/>
