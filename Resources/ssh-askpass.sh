#!/bin/zsh

prompt="${1:-SSH 비밀번호를 입력하세요}"

/usr/bin/osascript - "$prompt" <<'APPLESCRIPT'
on run argv
    set promptText to item 1 of argv
    set response to display dialog promptText default answer "" with hidden answer buttons {"취소", "확인"} default button "확인" cancel button "취소" with title "Remote OCP" with icon caution
    return text returned of response
end run
APPLESCRIPT
