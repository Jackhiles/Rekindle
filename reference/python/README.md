# Reference Python scripts

Early exploratory parsers kept as a structural reference for the Rust `ctf_loader` port.
They are not used by the build.

- `ccn.py`: walks the pack header and the top-level chunk list. Note the hard-coded game-data
  offset (`off=364544`); the Rust loader finds it from the PE headers instead.
- `list.py`: chunk id/flags/size histogram.
- `peek.py`: lists the sub-chunks of the first frame.
- `deep.py`: app strings, object definitions, per-frame stats, and the event-group walk
  (`ERev` marker → groups of `size i16, n_conditions u8, n_actions u8, ...`, conditions with a 16-byte
  header and actions with a 14-byte header, then parameters).
- `fileacts.py`: dumps condition/action parameters for extension events (optional frame-name filter as the 2nd arg).
- `usage.py`: expression token decoder (parameter codes 22/23/27/45). Useful for milestone 3.

Usage: `python ccn.py path/to/game.exe` (the other scripts `exec` `ccn.py` and expect the same argument).
