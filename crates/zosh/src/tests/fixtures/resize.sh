stty -echo
printf 'READY'
read -r line
printf '\rabcdefghijkl\r\nsecond-line\r\nthird-line\r\nprompt> '
read -r line
