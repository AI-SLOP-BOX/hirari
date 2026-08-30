/* Aura Quantum SDK - (c) 2026 */
#pragma once
struct P { virtual void p(float* l, float* r, int n)=0; virtual ~P(){} };
struct Host { virtual float* alloc(int s)=0; };
extern "C" { P* create(); void destroy(P* p); }
