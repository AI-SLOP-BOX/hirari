# Security policy

Aura is an early technical preview and processes untrusted project files,
audio files, extension manifests, and third-party plugins. Do not use it as a
security boundary for confidential material.

Please report suspected vulnerabilities privately to the maintainers through
the repository host's private security-advisory feature. Include affected
versions, reproduction steps, impact, and a minimal proof of concept. Do not
publish an exploitable report before a fix is available.

The current development branch receives security fixes. No older release line
is guaranteed to receive backports. Plugin binaries, FFmpeg, SDKs, voicebanks,
and operating-system components follow their vendors' security policies.

Never include credentials, signing identities, private projects, licensed
plugins, voicebanks, or user audio in a report.
