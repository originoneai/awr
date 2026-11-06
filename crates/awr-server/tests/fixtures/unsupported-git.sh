#!/bin/sh
# Simulate a Git release that predates show-ref --exists.
case "$*" in
    *--version*) printf 'git version 2.39.5 (Apple Git-128)\n' ;;
    *) exit 129 ;;
esac
