/* Process-local system resolver fixture for D0054. Only the three reserved
 * names below are intercepted; transport, egress classification and TLS are
 * untouched. No host resolver configuration or production code is changed. */
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
    int blocked = node && strcmp(node, "blocked.ldap-egress.test") == 0;
    int mixed = node && (strcmp(node, "mixed.ldap-egress.test") == 0 ||
                         strcmp(node, "wrong.ldap-egress.test") == 0);
    if (!blocked && !mixed) return original(node, service, hints, result);

    /* A reachable IPv6 unspecified destination comes first. The IPv6 trap
     * proves it is never dialed; IPv4 loopback is the allowed fallback in
     * integration-testing mode. All-blocked adds IPv4 unspecified instead. */
    const char *candidates[] = {"::", blocked ? "0.0.0.0" : "127.0.0.1"};
    struct addrinfo options;
    memset(&options, 0, sizeof(options));
    if (hints) options = *hints;
    options.ai_flags |= AI_NUMERICHOST;
    options.ai_flags &= ~AI_ADDRCONFIG;
    options.ai_family = AF_UNSPEC;
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
