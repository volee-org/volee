# The demo GIF of the user guide, recorded with vhs
# (https://github.com/charmbracelet/vhs). From the repository root:
#
#   vhs docs/tapes/demo.tape
#
# The terminal is 120 columns by 34 rows; the recording lasts about 16 seconds.

Output docs/src/images/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 1264
Set Height 704
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 150ms
Set Framerate 20
Set WaitTimeout 600s

# Build and start out of sight, until the interface is on screen.
Hide
Type "cargo run -q -p volee-cli -- demo"
Enter
Wait+Screen /need you/
Sleep 1s
Show

# The most urgent agents come first; the preview shows their screen.
Sleep 2s
Type "j"
Sleep 2s

# z shows that agent's whole screen; z again comes back.
Type "z"
Sleep 3s
Type "z"
Sleep 1s

# Two keys reach any agent: ' then its letter.
Type "'p"
Sleep 1.5s

# The keys, generated from the keymap.
Type "?"
Sleep 1.5s
Escape
Sleep 500ms

# Filter as you type.
Type "/trace"
Sleep 1s
Enter
Sleep 1s
Escape

# Enter shows one subtree with more detail.
Type "'l"
Enter
Sleep 2s
Escape
Sleep 500ms

Hide
Type "q"
Sleep 500ms
