/* Process-local system resolver fixture for D0057. Only reserved OIDC egress
 * test names are intercepted; transport, policy classification, TLS, host DNS,
 * and production code remain untouched. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <netdb.h>
#include <string.h>

typedef int (*resolve_fn)(const char *, const char *, const struct addrinfo *, struct addrinfo **);

static int fixture_resolve(const char *node, const char *service,
                           const struct addrinfo *hints, struct addrinfo **result) {
#ifdef __APPLE__
    resolve_fn original = getaddrinfo;
#else
    resolve_fn original = (resolve_fn)dlsym(RTLD_NEXT, "getaddrinfo");
#endif
    if (!original) return EAI_SYSTEM;

    int warmup = node && strcmp(node, "warmup.oidc-egress.test") == 0;
    int mixed = node && strcmp(node, "mixed.oidc-egress.test") == 0;
    int blocked = node && (
        strcmp(node, "blocked.oidc-egress.test") == 0 ||
        strcmp(node, "blocked-jwks.oidc-egress.test") == 0 ||
        strcmp(node, "blocked-token.oidc-egress.test") == 0);
    if (!warmup && !mixed && !blocked) return original(node, service, hints, result);

    /* Unspecified addresses are reachable traps in the scenario but must be
     * removed by EgressResolver. Mixed resolution retains loopback only in
     * explicit integration-testing mode; blocked resolution retains nothing. */
    const char *mixed_candidates[] = {"::", "127.0.0.1"};
    const char *blocked_candidates[] = {"::", "0.0.0.0"};
    const char **candidates = mixed ? mixed_candidates : blocked_candidates;
    struct addrinfo options;
    memset(&options, 0, sizeof(options));
    if (hints) options = *hints;
    options.ai_flags |= AI_NUMERICHOST;
    options.ai_flags &= ~AI_ADDRCONFIG;
    options.ai_family = AF_UNSPEC;
    if (warmup) return original("127.0.0.1", service, &options, result);
    *result = NULL;
    struct addrinfo **tail = result;
    for (unsigned i = 0; i < 2; i++) {
        struct addrinfo *part = NULL;
        int error = original(candidates[i], service, &options, &part);
        if (error) {
            if (*result) freeaddrinfo(*result);
            *result = NULL;
            return error;
        }
        *tail = part;
        while (*tail) tail = &(*tail)->ai_next;
    }
    return 0;
}

#ifdef __APPLE__
__attribute__((used)) static struct {
    const void *replacement;
    const void *original;
} interpose_getaddrinfo __attribute__((section("__DATA,__interpose"))) = {
    (const void *)fixture_resolve, (const void *)getaddrinfo
};
#else
int getaddrinfo(const char *node, const char *service,
                const struct addrinfo *hints, struct addrinfo **result) {
    return fixture_resolve(node, service, hints, result);
}
#endif
