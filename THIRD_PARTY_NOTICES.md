# Third-party notices

Fugue is MIT-licensed (see [`LICENSE`](LICENSE)). This file lists source
code from other projects that has been ported into Fugue, with each
project's copyright and license text, as those licenses require.

Entries are added as modules are ported, following
[DSP provenance](docs/dsp-provenance.md): every ported file names its
source in an attribution header, and its source appears here.

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

## Mutable Instruments Eurorack

- Source: https://github.com/gdamron/eurorack (a fork of
  pichenettes/eurorack), revision 08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4
- Ported:
  - `plaits/dsp/engine/virtual_analog_engine.cc` → `src/modules/macro_oscillator/engines/virtual_analog.rs`
  - `plaits/dsp/oscillator/variable_shape_oscillator.h` → `src/modules/macro_oscillator/oscillators/variable_shape.rs`
  - `plaits/dsp/oscillator/variable_saw_oscillator.h` → `src/modules/macro_oscillator/oscillators/variable_saw.rs`
- License: MIT

```
Copyright 2016 Emilie Gillet.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## Mutable Instruments stmlib

- Source: https://github.com/pichenettes/stmlib, revision
  e3bd7c9cc00e4364166f9905c0509b6ffd0535ec (the eurorack submodule pin)
- Ported: `dsp/polyblep.h` → `src/modules/macro_oscillator/engine.rs`
- License: MIT

```
Copyright 2017 Emilie Gillet.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```
