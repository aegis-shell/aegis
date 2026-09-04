# tessera-idle

`tessera-idle` is the standalone idle-policy coordinator for Tessera. It consumes
`ext-idle-notify-v1`, respects compositor-evaluated idle inhibitors, starts the
first-party locker with a secure readiness handshake, and coordinates later
display-power and suspend stages without moving idle policy into the
compositor.
