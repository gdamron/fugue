# Third-party notices

Fugue is MIT-licensed (see [`LICENSE`](LICENSE)). This file lists source
code from other projects that has been ported into Fugue, with each
project's copyright and license text, as those licenses require.

Fugue's own source carries no third-party code today. Entries are added as
modules are ported, following [DSP provenance](docs/dsp-provenance.md):
every ported file names its source in an attribution header, and its source
appears here.

Rust crate dependencies are not listed here: they are dependencies, not
vendored source, and binaries built from Fugue carry their notices
separately. Their licenses are checked in CI against
[`deny.toml`](deny.toml).

## Entry format

Each entry gives the project, where it came from, the files ported and
where they live in Fugue, then the full license text:

````text
## <Project name>

- Source: <repository URL>, revision <commit>
- Ported: <original path> → <Fugue path>
- License: <SPDX identifier>

```
<full license text, including the copyright line>
```
````
