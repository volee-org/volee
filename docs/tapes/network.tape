# The network view of the user guide, recorded with vhs
# (https://github.com/charmbracelet/vhs). From the repository root:
#
#   vhs docs/tapes/network.tape
#
# The terminal is 120 columns by 40 rows; the recording lasts about 16
# seconds. The demo's messages fly from its sixteenth second on.

Output docs/src/images/network.gif

Set Shell "bash"
Set FontSize 16
Set Width 1264
Set Height 824
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 150ms
Set Framerate 20
Set WaitTimeout 600s

# Build and start out of sight, open the network, and wait for the first
# messages.
Hide
Type "cargo run -q -p volee-cli -- demo"
Enter
Wait+Screen /need you/
Type "2"
Sleep 13s
Show

# trace-gateway writes to latency-audit: a pulse travels along the line.
Sleep 2500ms
Screenshot target/tapes/network_pulse.png

# The selection is shared with the list; its lines light up.
Type "'g"
Sleep 2s
Tab
Type "k"
Sleep 2s

# Less detail: dots with the jump letters, the selected agent named.
Type "_"
Sleep 2500ms
Screenshot target/tapes/network_dots.png
Type "0"
Sleep 3s
Screenshot target/tapes/network_end.png

Hide
Type "q"
Sleep 500ms
