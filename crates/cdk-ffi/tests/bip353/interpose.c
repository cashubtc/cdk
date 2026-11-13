// Test-only macOS shim: redirect CDK's DNS TCP sockets to a replay server,
// pin wall time for signed fixtures, and prohibit remote recipient connections.
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>

static int fixture_connect(int fd, const struct sockaddr *addr, socklen_t len) {
    if (addr->sa_family == AF_INET) {
        struct sockaddr_in target = *(const struct sockaddr_in *)addr;
        unsigned int ip = ntohl(target.sin_addr.s_addr);
        if ((ip == 0x08080808 || ip == 0x08080404) && ntohs(target.sin_port) == 53) {
            target.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
            target.sin_port = htons(atoi(getenv("CDK_DNS_PORT")));
            return connect(fd, (struct sockaddr *)&target, len);
        }
        if ((ip >> 24) == 127) return connect(fd, addr, len);
    } else if (addr->sa_family != AF_INET6) {
        return connect(fd, addr, len);
    }
    fprintf(stderr, "UNEXPECTED_REMOTE_CONNECT\n");
    errno = EPERM;
    return -1;
}

static int fixture_clock_gettime(clockid_t clock, struct timespec *ts) {
    if (clock == CLOCK_REALTIME) {
        ts->tv_sec = atoll(getenv("CDK_FIXTURE_TIME"));
        ts->tv_nsec = 0;
        return 0;
    }
    return clock_gettime(clock, ts);
}

static int fixture_gettimeofday(struct timeval *tv, void *tz) {
    tv->tv_sec = atoll(getenv("CDK_FIXTURE_TIME"));
    tv->tv_usec = 0;
    return 0;
}

#define INTERPOSE(replacement, original) \
    __attribute__((used)) static struct { const void *a; const void *b; } \
    interpose_##original __attribute__((section("__DATA,__interpose"))) = \
    { (const void *)&replacement, (const void *)&original };
INTERPOSE(fixture_connect, connect)
INTERPOSE(fixture_clock_gettime, clock_gettime)
INTERPOSE(fixture_gettimeofday, gettimeofday)
