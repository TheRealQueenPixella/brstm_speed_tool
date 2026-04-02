# brstm_speed_tool
A tool for speeding up .brstm files. Inspired by Atlas' Final Lap Stream Maker, but with the difference that this speeds up the input song file without increasing its pitch, making it perfect for creating sped-up versions of New Super Mario Bros. Wii custom music. It supports brstm files so it can be used for other games too, but it was created for NSMBW.
# Usage
This tool comes with a few commands:<br/>
`brstm_speed_tool.exe in.brstm out.brstm 1.15`<br/>
Where `in.brstm` is the name of the input file, and `out.brstm` is the name of the output file. `1.15` is the speed-up value, which, in the case of 1.15, speeds up the song by 15%;<br/>
`brstm_speed_tool.exe --loop-preview file.brstm`<br/>
This command plays the last 5 seconds of the song before the end loop and 5 seconds after the start loop. Replace `file.brstm` with the name of the file you want to listen to;<br/>
`brstm_speed_tool.exe in.brstm out.brstm --simple-resample 1.15`<br/>
This command does the same as the first one listed here, with the difference in the `--simple-resample` flag, that simply speeds up the song, also increasing the pitch, which is not ideal if you're looking to make a sped-up version for New Super Mario Bros. Wii.<br/>

### NOTE: you can drag and drop the file you want to speed up onto the brstm_speed_tool.exe file and it'll automatically create a 15% sped up version with no pitch change, akin to the first command listed. This feature was added in to make it easier to use without the need to use a command line<br/>

