# Thrax raylib demo

A small standalone project that drives [raylib](https://www.raylib.com/) from
Thrax through the **native (C) backend's FFI**.

## Run

```sh
clang++ -std=c++23 -I../../utilities build.cpp -o build   # bootstrap once
nix develop     # brings in raylib (exports $RAYLIB), cc and a display
./build run     # or just `build`
```
