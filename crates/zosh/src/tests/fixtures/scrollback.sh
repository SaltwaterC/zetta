printf 'old TUI\r\nREADY'
IFS= read -r reply || exit
# The request comes from terminal output, independently of any accelerator.
printf '\033[?25l\033[H\033[2Jnew prompt\033[3J\033[?25h' > /dev/tty
IFS= read -r reply || exit
# An ordinary redraw must preserve scrollback.
printf '\033[H\033[2Jredrawn prompt' > /dev/tty
IFS= read -r reply || exit
# A clear with no cell changes must still reach the client.
printf '\033[3J' > /dev/tty
IFS= read -r reply || exit
