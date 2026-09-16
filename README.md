# ethornell-rs

`ethornell-rs` is a reimplementation of the Ethornell / BURIKO General Interpreter (a.k.a. "BGI") visual novel engine.

The runtime detects the native game identifier from the comparison in
`system.arc:ipl._bp`. If detection is unavailable, the identifier falls back to
`Tayutama2TV`. For example:

```sh
cargo run -p ethornell-cli -- run --game /path/to/sun_flower
```

`--game` locates the resource archives, loose files, and user data.

For a GBK Chinese translation, also set `ETHORNELL_TEXT_ENCODING=gbk`.
This selects the encoding of text buffers passed to the graphics API, including
dialogue and speaker names. The default is Shift-JIS (`cp932`). It does not
change the encoding of resource names or BP system strings.
