/* The REPL's terminal, behind a handful of calls that hide termios (POSIX) and
 * the console API (Windows). The C headers carry each platform's struct layouts
 * and constants, which differ between Linux and macOS, so Rust never mirrors
 * them. One terminal per process: the saved settings are file statics. */

#ifdef _WIN32

#include <signal.h>
#include <windows.h>

static HANDLE g_in, g_out;
static DWORD g_in_orig, g_out_orig, g_in_raw;

int thx_term_enable(void) {
  g_in = GetStdHandle(STD_INPUT_HANDLE);
  g_out = GetStdHandle(STD_OUTPUT_HANDLE);
  if (!GetConsoleMode(g_in, &g_in_orig) || !GetConsoleMode(g_out, &g_out_orig)) return 0;
  /* Virtual-terminal input delivers keys as the same byte sequences a POSIX
   * terminal sends (arrows as ESC [ A, Return as CR), so one decoder serves. */
  g_in_raw = (g_in_orig & ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
             | ENABLE_VIRTUAL_TERMINAL_INPUT;
  if (!SetConsoleMode(g_in, g_in_raw)) return 0;
  SetConsoleMode(g_out, g_out_orig | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
  return 1;
}

void thx_term_enter(void) { SetConsoleMode(g_in, g_in_raw); }

void thx_term_leave(void) { SetConsoleMode(g_in, g_in_orig); }

int thx_term_read_byte(void) {
  unsigned char b;
  DWORD n = 0;
  if (!ReadFile(g_in, &b, 1, &n, NULL) || n != 1) return -1;
  return b;
}

/* Only key-down events produce bytes; focus, mouse, resize and key-up events
 * also signal the handle, so they are drained rather than reported. */
int thx_term_pending(int timeout_ms) {
  DWORD deadline = GetTickCount() + (DWORD)timeout_ms;
  for (;;) {
    DWORD left = deadline - GetTickCount();
    if ((int)left < 0) left = 0;
    if (WaitForSingleObject(g_in, left) != WAIT_OBJECT_0) return 0;
    INPUT_RECORD rec;
    DWORD n = 0;
    if (!PeekConsoleInputA(g_in, &rec, 1, &n) || n == 0) return 0;
    if (rec.EventType == KEY_EVENT && rec.Event.KeyEvent.bKeyDown &&
        rec.Event.KeyEvent.uChar.AsciiChar != 0)
      return 1;
    ReadConsoleInputA(g_in, &rec, 1, &n);
  }
}

int thx_term_rows(void) {
  CONSOLE_SCREEN_BUFFER_INFO info;
  if (!GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &info)) return 0;
  return info.srWindow.Bottom - info.srWindow.Top + 1;
}

int thx_term_err_cols(void) {
  CONSOLE_SCREEN_BUFFER_INFO info;
  if (!GetConsoleScreenBufferInfo(GetStdHandle(STD_ERROR_HANDLE), &info)) return 0;
  return info.srWindow.Right - info.srWindow.Left + 1;
}

static void (*g_on_interrupt)(void);

static void on_sigint(int sig) {
  signal(sig, on_sigint);
  g_on_interrupt();
}

void thx_term_catch_interrupts(void (*on_interrupt)(void)) {
  g_on_interrupt = on_interrupt;
  signal(SIGINT, on_sigint);
}

/* A Windows console has no job control. */
int thx_term_suspend(void) { return 0; }

#else

#include <poll.h>
#include <signal.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static struct termios g_orig, g_raw;

/* Echo, canonical line buffering and signal keys go off, so keys arrive one at
 * a time and Ctrl-C/D come through as bytes. `ICRNL` is cleared too, so Return
 * arrives as CR and `Ctrl-J` as LF. Output post-processing stays on. */
int thx_term_enable(void) {
  if (tcgetattr(STDIN_FILENO, &g_orig) != 0) return 0;
  g_raw = g_orig;
  g_raw.c_iflag &= ~(tcflag_t)ICRNL;
  g_raw.c_lflag &= ~(tcflag_t)(ISIG | ICANON | ECHO | IEXTEN);
  g_raw.c_cc[VMIN] = 1;
  g_raw.c_cc[VTIME] = 0;
  return tcsetattr(STDIN_FILENO, TCSANOW, &g_raw) == 0;
}

void thx_term_enter(void) { tcsetattr(STDIN_FILENO, TCSANOW, &g_raw); }

void thx_term_leave(void) { tcsetattr(STDIN_FILENO, TCSANOW, &g_orig); }

/* An unbuffered read straight from the fd, so `thx_term_pending`'s poll sees
 * exactly what is pending. */
int thx_term_read_byte(void) {
  unsigned char b;
  return read(STDIN_FILENO, &b, 1) == 1 ? b : -1;
}

int thx_term_pending(int timeout_ms) {
  struct pollfd pfd = {STDIN_FILENO, POLLIN, 0};
  return poll(&pfd, 1, timeout_ms) > 0;
}

int thx_term_rows(void) {
  struct winsize ws;
  memset(&ws, 0, sizeof ws);
  return ioctl(STDIN_FILENO, TIOCGWINSZ, &ws) == 0 ? ws.ws_row : 0;
}

int thx_term_err_cols(void) {
  struct winsize ws;
  memset(&ws, 0, sizeof ws);
  return ioctl(STDERR_FILENO, TIOCGWINSZ, &ws) == 0 ? ws.ws_col : 0;
}

static void (*g_on_interrupt)(void);

static void on_sigint(int sig) {
  (void)sig;
  g_on_interrupt();
}

/* `SA_RESTART` keeps a syscall the evaluated program is blocked in from failing
 * with `EINTR`. */
void thx_term_catch_interrupts(void (*on_interrupt)(void)) {
  struct sigaction act;
  memset(&act, 0, sizeof act);
  g_on_interrupt = on_interrupt;
  act.sa_handler = on_sigint;
  act.sa_flags = SA_RESTART;
  sigemptyset(&act.sa_mask);
  sigaction(SIGINT, &act, NULL);
}

int thx_term_suspend(void) { return raise(SIGTSTP) == 0; }

#endif
