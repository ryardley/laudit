#!/usr/bin/env bash

greet() {
  echo "hello $1"
}

NAME="world"
greet "$NAME"
source ./lib.sh
