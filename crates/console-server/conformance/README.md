# Console conformance

Runs Microsoft's own console feature tests against console-server: the
`Host.FeatureTests` suite (`src/host/ft_host` in
[microsoft/terminal](https://github.com/microsoft/terminal)), 335 tests of the
console API as a client sees it. They are not copied here; the script fetches
the pinned release and builds them.

```sh
make conformance-windows                       # from the repository root
make conformance-windows CONFORMANCE_ARGS="-Select 'OutputTests::*'"
```

or directly, from any directory:

```powershell
crates\console-server\conformance\conformance.ps1 [-Backend conpty] [-Select <TAEF name>] [-Source <checkout>] [-Update]
```

It needs Windows, git, Visual Studio 2022 or later with the C++ x64 tools and a
Windows SDK, and network access the first time. The first run takes a while:
it clones the source and builds the tests. Later runs rebuild only what changed
and take about five minutes.

## What it does

1. Clones microsoft/terminal at `v1.24.12741.0` into `target/conformance` and
   applies [`InitTests.patch`](InitTests.patch). The tests attach to the client
   their console host starts by taking the first other process in its job; a
   pseudoconsole host joins that job too, so the patch picks `Nihilist.exe` by
   name. Without it the ConPTY comparison attaches to the wrong process.
2. Installs the two NuGet packages the tests build against (TAEF and WIL) with
   the checkout's own `nuget.exe`.
3. Builds `Nihilist.exe` (the client) and `ConHost.Feature.Tests.dll` with the
   installed Visual Studio's toolset and newest SDK.
4. Builds [`examples/fthost.rs`](../examples/fthost.rs), which stands in for
   `OpenConsole.exe`: the tests start `OpenConsole.exe Nihilist.exe`, and
   fthost hosts Nihilist in a 120x30 terminal on console-server, the way a
   Zetta pane does. `-Backend conpty` hosts it on the inbox ConPTY instead.
5. Runs the suite with TAEF, with a one-minute timeout per test, and compares
   the result with
   [`expected-pass.txt`](expected-pass.txt). The script exits 1 if a listed
   test does not pass, and names any unlisted test that now passes.

The tests set and restore `HKCU\Console\ForceV2` while they run.

## The expected results

`expected-pass.txt` holds the 251 tests console-server passes, which are
exactly the ones inbox ConPTY passes. The other 84 behave the same under both
pseudoconsole hosts:

- 39 fail, and pass only in a windowed conhost: 30 `KeyPressTests` deliver
  window messages to the console window, and the rest (dimensions, fonts, one
  DBCS and two input tests, one in-flight message test) need a real window.
- 37 skip themselves, and run only in a windowed conhost: DBCS tests that need
  a raster font, and one font test.
- 8 skip themselves everywhere.

When a change makes another test pass, `-Update` rewrites the list from a full
console-server run. Review the diff before committing it, because a test can
also disappear from the list by failing.

`target/conformance/<backend>.log` keeps TAEF's output, and `fthost.log` in the
same directory keeps the host's own messages.
