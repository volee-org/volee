# The repositories view of the user guide, recorded with vhs
# (https://github.com/charmbracelet/vhs). From the repository root:
#
#   vhs docs/tapes/repos.tape
#
# The terminal is 120 columns by 40 rows; the recording lasts about 15
# seconds. Pull request #42 merges at the demo's 26th second.

Output docs/src/images/repos.gif

Set Shell "bash"
Set FontSize 16
Set Width 1264
Set Height 824
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 150ms
Set Framerate 20
Set WaitTimeout 600s

# Build and start out of sight, then open the repositories.
Hide
Type "cargo run -q -p volee-cli -- demo"
Enter
Wait+Screen /need you/
Type "3"
Sleep 18s
Show

# Every agent's delivery evidence, and the supervisor's verdict.
Sleep 2s
Tab
Type "'p"
Sleep 2s
Screenshot target/tapes/repos_before.png

# Pull request #42 merges: its worktree becomes safe to tidy.
Sleep 4s
Screenshot target/tapes/repos_merged.png

# More detail: a line for the evidence, and why a worktree is kept.
Type "+"
Sleep 2500ms
Screenshot target/tapes/repos_full.png
Type "l"
Sleep 1500ms
Type "l"
Sleep 2s

Hide
Type "q"
Sleep 500ms
