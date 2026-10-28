# Menu audio

`pausemenu.mp3` is the streaming asset used on the GLUE 5 start page. It was converted from the user-provided `pausemenu.m4a` (AAC-LC, 48 kHz stereo, about 21:28) to 48 kHz stereo MP3 at 160 kbit/s so SDL3_mixer can decode it on supported targets. The original M4A was left unchanged in Google Drive.

The application embeds and streams this MP3; it does not decode the full track into PCM at startup.
