# ethornell-rs

`ethornell-rs` is a reimplementation of the Ethornell / BURIKO General Interpreter (a.k.a. "BGI") visual novel engine.

The native game identifier defaults to `Tayutama2TV`. Set `ETHORNELL_GAME_ID`
to the identifier expected by another game's bootstrap script; a mismatch can
terminate the script before it draws anything. For 向日葵の教会と長い夏休み:

```sh
ETHORNELL_GAME_ID=HimawariNoKyoukaiToNagaiNatsuyasumi \
ETHORNELL_NATIVE_ROOT=/path/to/sun_flower \
cargo run -p ethornell-cli -- run --game /path/to/sun_flower
```

`--game` locates the resource archives. `ETHORNELL_NATIVE_ROOT` locates loose
files and user data, and defaults to the current working directory.
