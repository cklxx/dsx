# App-server daemon

The daemon manages a local DSX app-server process through a pid file and Unix control socket.

Commands:

```text
dsx app-server daemon start
dsx app-server daemon restart
dsx app-server daemon stop
dsx app-server daemon version
```

The managed executable path is:

```text
$DSX_HOME/packages/standalone/current/dsx
```

The daemon does not install or update DSX. The executable must already exist at that path.
