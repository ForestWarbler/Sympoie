#!/usr/bin/env python3
"""Builds the Calor plugin and bundles it as Calor.vst3 and Calor.clap in target/bundled.

    python3 tools/bundle_calor.py [--debug] [--universal] [--install]

The options are bundle_reson.py's: this runs its steps with Calor's names.
"""
import bundle_reson as bundle

bundle.NAME = "Calor"
bundle.PACKAGE = "calor-plugin"
bundle.LIBRARY = "calor"
bundle.IDENTIFIER = "com.sympoie.calor"

if __name__ == "__main__":
    bundle.main()
