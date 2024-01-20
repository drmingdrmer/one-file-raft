#!/bin/sh

cat ./src/lib.rs \
    | grep -v '^$' \
    | grep -v '^ *//' \
    | grep -v '^ *#' \
    | grep -v '^ *trace' \
    | grep -v '^ *debug' \
    | grep -v '^ *info' \
    | grep -v '^ *assert' \
    | grep -v '^ *use ' \
    | wc -l
