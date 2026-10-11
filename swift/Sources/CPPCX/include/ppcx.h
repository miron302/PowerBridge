#ifndef PPCX_H
#define PPCX_H
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct PpcxSession PpcxSession;

/* kind: 0 = stdout, 1 = stderr, 2 = log line */
typedef void (*ppcx_event_cb)(int32_t kind, const uint8_t *data, size_t len, void *user);

PpcxSession *ppcx_session_new(void);
void ppcx_session_free(PpcxSession *s);
void ppcx_session_stop(PpcxSession *s);
int32_t ppcx_session_exit_code(PpcxSession *s);
char *ppcx_session_message(PpcxSession *s);                     /* free with ppcx_string_free */
void ppcx_session_regs(PpcxSession *s, uint32_t *out, size_t n); /* r0..r31, pc, lr, ctr, cr, xer, insns_hi, insns_lo */

/* Blocking. 0 exited, 1 stopped, 2 instruction limit, 3 crashed, -1 failed to load. `path` may be an .app bundle. */
int32_t ppcx_run(PpcxSession *s, const char *path, const char *const *args, size_t nargs,
                 uint64_t max_insns, int32_t trace_sys, int32_t trace_imp,
                 ppcx_event_cb cb, void *user);

char *ppcx_analyze_json(const char *path);                       /* free with ppcx_string_free */
/* Unpack a .pkg/.mpkg into dest_dir; JSON {apps, binaries, files, bytes, warnings} or {error}. Free with ppcx_string_free. */
char *ppcx_pkg_extract_json(const char *pkg_path, const char *dest_dir);
int32_t ppcx_wrap_app(const char *src, const char *dest_dir, const char *ppcx_bin, char **out);
void ppcx_string_free(char *p);

#ifdef __cplusplus
}
#endif
#endif
