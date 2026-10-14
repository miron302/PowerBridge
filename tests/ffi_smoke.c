/* Exercises the same C ABI the Swift app uses. Build/run: scripts/test-ffi.sh */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ppcx.h"

static char out[4096]; static size_t outlen; static int logs;
static void cb(int32_t kind, const uint8_t *d, size_t n, void *user) {
    if (kind == 0 && outlen + n < sizeof out) { memcpy(out + outlen, d, n); outlen += n; }
    if (kind == 2) logs++;
    (void)user;
}
#define CHECK(c) do { if (!(c)) { printf("FAIL line %d: %s\n", __LINE__, #c); return 1; } } while (0)

int main(int argc, char **argv) {
    const char *root = argc > 1 ? argv[1] : ".";
    char app[1024], tool[1024], cocoa[1024], dest[] = "/tmp/ppcx-ffi-XXXXXX";
    snprintf(app, sizeof app, "%s/tests/fixtures/apps/Hello.app", root);
    snprintf(tool, sizeof tool, "%s/tests/fixtures/apps/Bin Tool.app", root);
    snprintf(cocoa, sizeof cocoa, "%s/tests/fixtures/apps/Cocoa.app", root);

    char *j = ppcx_analyze_json(app);
    CHECK(strstr(j, "\"verdict\":\"runs\"") != NULL);
    ppcx_string_free(j);
    j = ppcx_analyze_json(cocoa);
    CHECK(strstr(j, "\"verdict\":\"unsupported\"") != NULL);
    ppcx_string_free(j);
    j = ppcx_analyze_json("/nonexistent");
    CHECK(strstr(j, "\"error\"") != NULL);
    ppcx_string_free(j);

    PpcxSession *s = ppcx_session_new();
    int32_t r = ppcx_run(s, app, NULL, 0, 0, 1, 1, cb, NULL);
    CHECK(r == 0);
    CHECK(ppcx_session_exit_code(s) == 7);
    CHECK(strcmp(out, "sum=55 fact=3628800 hex=0xff str=PowerPC\n") == 0);
    CHECK(logs > 0); /* import tracing produced log events */
    char *msg = ppcx_session_message(s);
    CHECK(strstr(msg, "Exited with status 7") != NULL);
    ppcx_string_free(msg);
    uint32_t regs[40]; memset(regs, 0, sizeof regs);
    ppcx_session_regs(s, regs, 40);
    CHECK(regs[38] > 50); /* instruction count (lo word) */
    printf("ran %u guest instructions\n", regs[38]);

    /* session is reusable; bad path reports a load error */
    r = ppcx_run(s, "/nonexistent.app", NULL, 0, 0, 0, 0, cb, NULL);
    CHECK(r == -1);
    msg = ppcx_session_message(s);
    CHECK(strlen(msg) > 0);
    printf("load error message: %s\n", msg);
    ppcx_string_free(msg);

    /* not-PowerPC input */
    r = ppcx_run(s, "/bin/sh", NULL, 0, 0, 0, 0, cb, NULL);
    CHECK(r == -1);
    ppcx_session_free(s);

    /* wrapper */
    CHECK(mkdtemp(dest) != NULL);
    char ppcx_bin[1100]; snprintf(ppcx_bin, sizeof ppcx_bin, "%s/target/release/ppcx", root);
    char *res = NULL;
    CHECK(ppcx_wrap_app(tool, dest, ppcx_bin, &res) == 0);
    printf("wrapped: %s\n", res);
    CHECK(strstr(res, "Bin Tool (PowerBridge).app") != NULL);
    ppcx_string_free(res);
    CHECK(ppcx_wrap_app("/nonexistent.app", dest, ppcx_bin, &res) == 1);
    printf("wrap error: %s\n", res);
    ppcx_string_free(res);

    /* installer packages */
    char pk[1024], pkdest[] = "/tmp/ppcx-ffi-pkg-XXXXXX", bz[1024];
    snprintf(pk, sizeof pk, "%s/tests/fixtures/pkgs/flat_product.pkg", root);
    snprintf(bz, sizeof bz, "%s/tests/fixtures/pkgs/bz.pkg", root);
    CHECK(mkdtemp(pkdest) != NULL);
    j = ppcx_pkg_extract_json(pk, pkdest);
    CHECK(strstr(j, "Cocoa.app") != NULL && strstr(j, "Hello.app") != NULL);
    ppcx_string_free(j);
    j = ppcx_pkg_extract_json(bz, pkdest);
    CHECK(strstr(j, "\"error\"") != NULL && strstr(j, "bzip2") != NULL);
    printf("package error: %s\n", j);
    ppcx_string_free(j);

    printf("FFI smoke test OK\n");
    return 0;
}
