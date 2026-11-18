# shellcheck shell=bash
# Shared by win-perf.sh and win-do.sh (sourced). Reads `quser` output from the Windows PC.

# "USER IDLE" of the session whose SESSIONNAME is exactly "console", or nothing. Columns are found from the
# header, so a user name containing "console", an RDP session, a disconnected session (no SESSIONNAME), a 12-hour
# LOGON TIME with AM/PM and the ">" marker of the current session are all handled.
quser_console() {
  awk '{ sub(/\r$/, "") }
    NR == 1 { s = index($0, "SESSIONNAME"); st = index($0, "STATE"); lt = index($0, "LOGON TIME"); next }
    s && st && lt {
      user = substr($0, 1, s - 1); sub(/^[ >]+/, "", user); sub(/ +$/, "", user)
      n = split(substr($0, s, st - s), sess, " ")      # "console 1", or only the ID when there is no session name
      m = split(substr($0, st, lt - st), si, " ")      # "Active 5", "Disc 1+19:29"
      if (n == 2 && sess[1] == "console" && m >= 2) print user, si[m]
    }'
}

# Minutes from quser's IDLE TIME ("1+19:29" days+h:mm, "1:05" h:mm, "5" minutes, "." or "none" = none).
idle_minutes() {
  case $1 in
    *+*) local d=${1%%+*} r=${1#*+}; echo $((10#$d * 1440 + $(idle_minutes "$r"))) ;;
    *:*) echo $((10#${1%%:*} * 60 + 10#${1#*:})) ;;
    .|none|'') echo 0 ;;
    *) echo $((10#$1)) ;;
  esac
}
