# The timeline view of the user guide, recorded with vhs
# (https://github.com/charmbracelet/vhs). From the repository root:
#
#   vhs docs/tapes/timeline.tape
#
# The terminal is 120 columns by 40 rows; the recording lasts about 16
# seconds.

Output docs/src/images/timeline.gif

Set Shell "bash"
Set FontSize 16
Set Width 1264
Set Height 824
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 150ms
Set Framerate 20
Set WaitTimeout 600s

# Build and start out of sight, then open the timeline.
Hide
Type "cargo run -q -p volee-cli -- demo"
Enter
Wait+Screen /need you/
Type "4"
Sleep 1s
Show

# A lane per agent: states as bars, messages and deliveries as marks.
Sleep 2s
Tab
Type "'v"
Sleep 1500ms

# Back through invoice-tools' events: the line below reads the cursor.
Type "["
Sleep 1500ms
Type "["
Sleep 2s
Screenshot target/tapes/timeline_scrubbed.png
Type "hhhh"
Sleep 1500ms
Type "]"
Sleep 1500ms

# A finer time scale, then back to the present.
Escape
Type "++"
Sleep 2500ms
Screenshot target/tapes/timeline_zoomed.png
Type "0"
Sleep 2s

Hide
Type "q"
Sleep 500ms
