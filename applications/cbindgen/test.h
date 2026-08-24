// A small C header to exercise the binding generator.
#define UNUSED_MACRO 123
#include <stdio.h>

/* An enum: becomes an Int32 alias plus named constants. */
typedef enum { RED, GREEN = 5, BLUE } ColorKind;

typedef struct { int x; int y; } Point;

typedef struct {
    float r;
    float g;
    float b;
    float a;
} Colorf;

typedef union { int i; float f; } Word;

int add(int a, int b);
void draw_point(Point p, Colorf c);
Point make_point(int x, int y);
double scale(double v, float k);
const char *name_of(ColorKind kind);

typedef struct { float m[16]; } Matrix;
void set_pixels(Colorf *pixels, int count);
