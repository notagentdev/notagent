# Run with python3 scripts/demo/record.py from the repository root.
Output assets/demo/notagent.gif
Output assets/demo/notagent.mp4

Set Shell "bash"
Set FontFamily "Menlo"
Set FontSize 20
Set Width 1200
Set Height 800
Set Padding 24
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set WindowBarSize 32
Set BorderRadius 12
Set Framerate 20
Set TypingSpeed 35ms
Set CursorBlink false

Hide
Type `cd "$NOTAGENT_DEMO_PROJECT"; clear`
Enter
Type `"$NOTAGENT_DEMO_BINARY" --offline --no-session --no-skills --no-prompt-templates --no-context-files --no-approve --provider demo --model scripted-demo --tui-mode fullscreen`
Enter
Sleep 2s
Show
Sleep 1s

# Plan: the provider sends a real compact read through the agent loop.
Type "Free shipping at $50 still charges $4.99. Inspect and plan a fix."
Enter
Wait+Screen /Ready to implement/
Sleep 3s
Screenshot assets/demo/01-plan.png

# The built-in ring is plan -> manual -> auto.
Shift+Tab
Sleep 500ms
Shift+Tab
Sleep 1s
Type "Implement the plan and run the tests."
Enter
Wait+Screen /Fixed and verified/
Sleep 5s
Screenshot assets/demo/02-result.png
Sleep 200ms

Hide
Ctrl+c
Ctrl+c
Sleep 500ms
