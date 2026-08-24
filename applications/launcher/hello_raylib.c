/* Pure-C raylib window test (no Thrax). If no window appears here either,
   the issue is raylib/Wayland/GPU on this machine, not Thrax. */
#include <stdio.h>
#include "raylib.h"
int main(void) {
    InitWindow(640, 400, "C raylib hello");
    SetTargetFPS(60);
    while (!WindowShouldClose()) {
        BeginDrawing();
        ClearBackground((Color){30, 30, 46, 255});
        DrawText("If you can read this, raylib works.", 40, 180, 20,
                 (Color){232, 232, 240, 255});
        EndDrawing();
    }
    CloseWindow();
    printf("closed\n");
    return 0;
}
