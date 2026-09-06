# Security policy

Aura processes untrusted project files, audio files, extension manifests, and
third-party plugins. The application is not a security boundary: use normal
endpoint protection and do not open untrusted plugin binaries in a production
session.

Please report suspected vulnerabilities privately to the maintainers through
the repository host's private security-advisory feature. Include affected
versions, reproduction steps, impact, and a minimal proof of concept. Do not
publish an exploitable report before a fix is available.

The latest supported release line and the current development branch receive
security fixes. Older release lines are not guaranteed to receive backports.
Plugin binaries, FFmpeg, SDKs, voicebanks, and operating-system components
follow their vendors' security policies.

Never include credentials, signing identities, private projects, licensed
plugins, voicebanks, or user audio in a report.
