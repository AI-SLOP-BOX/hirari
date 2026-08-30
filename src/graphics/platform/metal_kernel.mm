#import <Metal/Metal.h>
#import <MetalKit/MetalKit.h>
#import <Cocoa/Cocoa.h>
#include <vector>
#include <string>
#include <iostream>
#include <cmath>
#include <simd/simd.h>
#include <array>
#include "../graphics_kernel.hpp"

namespace Aura::Graphics::Platform {

struct Uniforms {
    simd_float4 pos;     // xy, zw
    simd_float4 props;   // p1, p2, p3, type
    simd_float4 color1;  // RGBA
    simd_float4 color2;  // RGBA
};

class MetalKernel : public IGraphicsKernel {
public:
    static constexpr uint32_t kMaxInstances = 131072;

    MetalKernel() : m_view(nil), m_device(nil), m_commandQueue(nil), m_bufferIndex(0), m_scale(1.0f) {
        m_semaphore = dispatch_semaphore_create(3);
    }

    bool initialize(void* viewHandle) override {
        m_view = (__bridge MTKView*)viewHandle;
        if (!m_view) return false;
        m_device = m_view.device;
        m_commandQueue = [m_device newCommandQueue];
        
        id<MTLLibrary> library = [m_device newDefaultLibrary];
        if (!library) {
            NSString* path = [[NSBundle mainBundle] pathForResource:@"default" ofType:@"metallib"];
            if (path) library = [m_device newLibraryWithFile:path error:nil];
        }
        if (!library) {
            std::cerr << "[MetalKernel] Critical Error: Failed to load shader library (metallib missing?)" << std::endl;
            return false;
        }

        MTLRenderPipelineDescriptor* pd = [[MTLRenderPipelineDescriptor alloc] init];
        pd.vertexFunction = [library newFunctionWithName:@"vUI"];
        pd.fragmentFunction = [library newFunctionWithName:@"fUI"];
        
        if (!pd.vertexFunction || !pd.fragmentFunction) {
            std::cerr << "[MetalKernel] Critical Error: Shader functions 'vUI' or 'fUI' not found in library." << std::endl;
            return false;
        }
        
        pd.colorAttachments[0].pixelFormat = m_view.colorPixelFormat;
        pd.colorAttachments[0].blendingEnabled = YES;
        pd.colorAttachments[0].sourceRGBBlendFactor = MTLBlendFactorSourceAlpha;
        pd.colorAttachments[0].destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        
        NSError* error = nil;
        m_pipelineState = [m_device newRenderPipelineStateWithDescriptor:pd error:&error];
        if (!m_pipelineState) {
            std::cerr << "[MetalKernel] Failed to create pipeline state: " << [[error localizedDescription] UTF8String] << std::endl;
            return false;
        }
        
        for (int i = 0; i < 3; ++i) {
            m_uniformBuffers[i] = [m_device newBufferWithLength:kMaxInstances * sizeof(Uniforms) options:MTLResourceStorageModeShared];
            if (!m_uniformBuffers[i]) return false;
        }
        return true;
    }


    void beginFrame() override {
        if (!m_view || !m_pipelineState) return;
        dispatch_semaphore_wait(m_semaphore, DISPATCH_TIME_FOREVER);
        m_commandBuffer = [m_commandQueue commandBuffer];
        m_encoder = nil;
        m_renderPassDescriptor = nil;
        __block dispatch_semaphore_t s = m_semaphore;
        [m_commandBuffer addCompletedHandler:^(id<MTLCommandBuffer>){ dispatch_semaphore_signal(s); }];
        m_renderPassDescriptor = m_view.currentRenderPassDescriptor;
        if (m_renderPassDescriptor) {
            m_encoder = [m_commandBuffer renderCommandEncoderWithDescriptor:m_renderPassDescriptor];
            [m_encoder setRenderPipelineState:m_pipelineState];
        }
        m_bufferIndex = (m_bufferIndex + 1) % 3;
        m_instanceCount = 0;
        m_scissorDepth = 0;
        m_mappedUniforms = (Uniforms*)[m_uniformBuffers[m_bufferIndex] contents];
    }

    void endFrame() override {
        if (!m_commandBuffer) return;
        if (m_encoder && m_instanceCount > 0) {
            [m_encoder setVertexBuffer:m_uniformBuffers[m_bufferIndex] offset:0 atIndex:0];
            [m_encoder drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4 instanceCount:m_instanceCount];
            [m_encoder endEncoding];
        }
        id<CAMetalDrawable> drawable = m_view.currentDrawable;
        if (drawable) [m_commandBuffer presentDrawable:drawable];
        [m_commandBuffer commit];
        m_commandBuffer = nil;
        m_encoder = nil;
    }

    void drawNative(float x, float y, float w, float h, float p1, float p2, float p3, uint32_t c1, uint32_t c2, float type) override {
        appendNative(x, y, w, h, p1, p2, p3, c1, c2, type);
    }

    void drawRoundedRect(float x, float y, float w, float h, float r, uint32_t c) override { drawNative(x,y,w,h,r,0,0,c,c,0); }
    void drawRect(float x, float y, float w, float h, uint32_t c) override { drawNative(x,y,w,h,0,0,0,c,c,0); }
    void drawGradientRect(float x, float y, float w, float h, uint32_t c1, uint32_t c2) override { drawNative(x,y,w,h,0,0,0,c1,c2,0); }
    void drawCircle(float x, float y, float r, uint32_t c) override { drawNative(x-r, y-r, r*2, r*2, 1.0, 0, 0, c, c, 0); }
    void drawLine(float x1, float y1, float x2, float y2, float t, uint32_t c) override {
        const float dx = x2 - x1, dy = y2 - y1;
        const float length = std::sqrt(dx * dx + dy * dy);
        if (!std::isfinite(length) || length <= 0.01f) { drawCircle(x1, y1, std::max(.5f, t*.5f), c); return; }
        const uint32_t steps = std::min<uint32_t>(256, std::max<uint32_t>(1, static_cast<uint32_t>(std::ceil(length / 2.0f))));
        const float radius = std::max(.5f, t * .5f);
        for (uint32_t i = 0; i <= steps; ++i) {
            const float p = static_cast<float>(i) / steps;
            drawCircle(x1 + dx * p, y1 + dy * p, radius, c);
        }
    }
    void drawText(const std::string& t, float x, float y, float s, uint32_t c) override {
        // Text glyphs are appended directly to the same instance buffer. The
        // frame is submitted once in endFrame, so a label does not create a
        // command-buffer draw call per character.
        float cx = x;
        for (const unsigned char ch : t) {
            if (ch == '\n') { cx = x; y += s; continue; }
            if (ch != ' ') appendNative(cx, y, s * 0.6f, s, static_cast<float>(ch), 0, 0, c, c, 4.0f);
            cx += s * 0.48f;
        }
    }
    float measureText(const std::string& t, float s) const override { return t.length()*s*0.48f; }
    void drawNeonRect(float x, float y, float w, float h, float r, float g, uint32_t c) override { drawNative(x,y,w,h,r,g,0,c,c,0); }
    void drawGlassRect(float x, float y, float w, float h, float r, uint32_t c) override { drawNative(x,y,w,h,r,0,0,c,c,3); }
    void drawMeter(uint32_t, float l, float r, float x, float y, float w, float h) override {
        drawNative(x,y,w,h,2,0,0,0xFF141416,0xFF141416,0);
        float hL = h*std::clamp(l,0.f,1.f), hR = h*std::clamp(r,0.f,1.f);
        drawNative(x+1, y+h-hL, w*0.42f, hL, 1,0,0,0xFF30B0FF,0xFFAABBCC,0);
        drawNative(x+w*0.52f, y+h-hR, w*0.42f, hR, 1,0,0,0xFF30B0FF,0xFFAABBCC,0);
    }
    void drawWaveformPath(const float* mn, const float* mx, size_t n, float x, float cy, float w, float h, uint32_t c) override {
        if (!mn || !mx || n == 0 || !std::isfinite(x) || !std::isfinite(cy) ||
            !std::isfinite(w) || !std::isfinite(h) || w <= 0.0f || h <= 0.0f) return;
        const size_t visible = std::min<size_t>(n, kMaxInstances);
        const float step = w / static_cast<float>(visible);
        for (size_t i = 0; i < visible; i += 4) {
            const size_t sample = std::min(i, visible - 1);
            if (!std::isfinite(mn[sample]) || !std::isfinite(mx[sample])) continue;
            const float lo = std::min(mn[sample], mx[sample]);
            const float hi = std::max(mn[sample], mx[sample]);
            const float top = cy + lo * h;
            const float height = std::max(1.0f, (hi - lo) * h);
            drawNative(x + static_cast<float>(sample) * step, top,
                       std::max(1.0f, step), height, 0, 0, 0, c, c, 0);
        }
    }

    // Interface operations backed by the active Metal encoder.
    void setScale(float s) override { m_scale = s; }
    float getScale() const override { return m_scale; }
    void pushScissor(float x, float y, float w, float h) override {
        if (!m_encoder || m_scissorDepth >= m_scissors.size()) return;
        m_scissors[m_scissorDepth++] = {std::max(0.0f, x), std::max(0.0f, y),
                                        std::max(0.0f, w), std::max(0.0f, h)};
        const CGSize size = m_view.drawableSize;
        const NSUInteger sx = static_cast<NSUInteger>(std::clamp(x * m_scale, 0.0f, static_cast<float>(size.width)));
        const NSUInteger sy = static_cast<NSUInteger>(std::clamp(y * m_scale, 0.0f, static_cast<float>(size.height)));
        const NSUInteger sw = static_cast<NSUInteger>(std::clamp(w * m_scale, 0.0f, static_cast<float>(size.width) - sx));
        const NSUInteger sh = static_cast<NSUInteger>(std::clamp(h * m_scale, 0.0f, static_cast<float>(size.height) - sy));
        [m_encoder setScissorRect:(MTLScissorRect){sx, sy, sw, sh}];
    }
    void popScissor() override {
        if (!m_encoder || m_scissorDepth == 0) return;
        --m_scissorDepth;
        if (m_scissorDepth == 0) {
            const CGSize size = m_view.drawableSize;
            [m_encoder setScissorRect:(MTLScissorRect){0, 0, (NSUInteger)size.width, (NSUInteger)size.height}];
        } else {
            const auto& s = m_scissors[m_scissorDepth - 1];
            const CGSize size = m_view.drawableSize;
            const NSUInteger x = static_cast<NSUInteger>(std::clamp(s.x * m_scale, 0.0f, static_cast<float>(size.width)));
            const NSUInteger y = static_cast<NSUInteger>(std::clamp(s.y * m_scale, 0.0f, static_cast<float>(size.height)));
            const NSUInteger w = static_cast<NSUInteger>(std::clamp(s.w * m_scale, 0.0f, static_cast<float>(size.width) - x));
            const NSUInteger h = static_cast<NSUInteger>(std::clamp(s.h * m_scale, 0.0f, static_cast<float>(size.height) - y));
            [m_encoder setScissorRect:(MTLScissorRect){x, y, w, h}];
        }
    }
    void drawVertexPath(const Vertex* v, size_t n, float t) override {
        if (!v || n == 0) return;
        for (size_t i = 1; i < n; ++i) drawLine(v[i-1].x, v[i-1].y, v[i].x, v[i].y, t, v[i].color);
    }
    void drawVertexPathFilled(const Vertex* v, size_t n) override {
        if (!v || n < 3) return;
        for (size_t i = 2; i < n; ++i)
            drawFilledTriangle(v[0].x, v[0].y, v[i-1].x, v[i-1].y, v[i].x, v[i].y, v[i].color);
    }
    void updateSpectrogram(const std::vector<float>& data) override {
        if (data.empty() || !m_encoder || !m_view) return;
        const float width = static_cast<float>(m_view.drawableSize.width) / std::max(1.0f, m_scale);
        const float height = static_cast<float>(m_view.drawableSize.height) / std::max(1.0f, m_scale);
        const size_t visible = std::min<size_t>(data.size(), 256);
        const float barW = width / static_cast<float>(visible);
        for (size_t i = 0; i < visible; ++i) {
            const float level = std::clamp(std::isfinite(data[i]) ? data[i] : 0.0f, 0.0f, 1.0f);
            drawNative(static_cast<float>(i) * barW, height * (1.0f - level),
                       std::max(1.0f, barW - 1.0f), height * level, 0, 0, 0,
                       0xff2a82e4u, 0xff30a46cu, 0);
        }
    }
    void drawBrushedCircle(float x, float y, float r, uint32_t c) override { drawCircle(x,y,r,c); }
    void drawFilledPath(const std::vector<float>& p, uint32_t c) override {
        if (p.size() < 6 || (p.size() & 1u)) return;
        for (size_t i = 4; i + 1 < p.size(); i += 2)
            drawFilledTriangle(p[0], p[1], p[i-2], p[i-1], p[i], p[i+1], c);
    }
    void drawFilledTriangle(float x1, float y1, float x2, float y2, float x3, float y3, uint32_t c) override {
        const float minY = std::min({y1, y2, y3}), maxY = std::max({y1, y2, y3});
        const uint32_t rows = std::min<uint32_t>(128, std::max<uint32_t>(1, static_cast<uint32_t>(std::ceil(maxY - minY))));
        const auto hit = [](float ay, float ax, float by, float bx, float y, float& out) {
            if (ay == by || y < std::min(ay, by) || y > std::max(ay, by)) return false;
            out = ax + (y - ay) * (bx - ax) / (by - ay); return std::isfinite(out);
        };
        for (uint32_t row = 0; row < rows; ++row) {
            const float y = minY + (maxY - minY) * (row + 0.5f) / rows;
            float p[3]{}; uint32_t n = 0;
            n += hit(y1,x1,y2,x2,y,p[n]); n += hit(y2,x2,y3,x3,y,p[n]); n += hit(y3,x3,y1,x1,y,p[n]);
            if (n >= 2) drawNative(std::min(p[0],p[1]), y, std::max(1.0f, std::fabs(p[1]-p[0])),
                                   std::max(1.0f, (maxY-minY)/rows), 0,0,0,c,c,0);
        }
    }
    void drawDropShadow(float x, float y, float w, float h, float r, uint32_t c) override {
        const uint32_t passes = std::min<uint32_t>(8, std::max<uint32_t>(1, static_cast<uint32_t>(r)));
        for (uint32_t i = passes; i > 0; --i)
            drawRoundedRect(x - i * .5f, y - i * .5f, w + i, h + i, r + i,
                            (c & 0x00ffffffu) | (static_cast<uint32_t>(((c >> 24) & 255u) * (float)i / passes * .18f) << 24));
    }
    void applyBlurEffect(float x, float y, float w, float h, float intensity) override {
        const uint32_t passes = std::min<uint32_t>(6, std::max<uint32_t>(1, static_cast<uint32_t>(intensity * 4.0f)));
        for (uint32_t i = 1; i <= passes; ++i) drawRoundedRect(x-i,y-i,w+2*i,h+2*i,static_cast<float>(i),0x12000000u);
    }
    void drawBezierPath(const std::vector<float>& p, uint32_t c, float t) override {
        if (p.size() < 4 || (p.size() & 1u)) return;
        for (size_t i = 3; i < p.size(); i += 2) drawLine(p[i-3],p[i-2],p[i-1],p[i],t,c);
    }
    void drawBezierCurve(float x1,float y1,float cp1x,float cp1y,float cp2x,float cp2y,float x2,float y2,float t,uint32_t c) override {
        float px=x1, py=y1;
        for (int i=1;i<=32;++i) { float ox,oy; calculateBezier(x1,y1,cp1x,cp1y,cp2x,cp2y,x2,y2,i/32.0f,ox,oy); drawLine(px,py,ox,oy,t,c); px=ox;py=oy; }
    }
    void calculateBezier(float x1,float y1,float cp1x,float cp1y,float cp2x,float cp2y,float x2,float y2,float t,float& ox,float& oy) const override {
        const float u=1.0f-t; ox=u*u*u*x1+3*u*u*t*cp1x+3*u*t*t*cp2x+t*t*t*x2; oy=u*u*u*y1+3*u*u*t*cp1y+3*u*t*t*cp2y+t*t*t*y2;
    }
    void drawArc(float cx,float cy,float r,float start,float end,float thick,uint32_t c) override {
        const float span=end-start; const uint32_t n=std::min<uint32_t>(256,std::max<uint32_t>(2,static_cast<uint32_t>(std::ceil(std::fabs(span)/8.0f))));
        for(uint32_t i=0;i<=n;++i){const float a=(start+span*i/n)*3.14159265358979323846f/180.0f;drawCircle(cx+std::cos(a)*r,cy+std::sin(a)*r,std::max(.5f,thick*.5f),c);}
    }
    void drawIconAudio(float x, float y, float s, uint32_t c) override { drawNative(x,y,s,s,65,0,0,c,c,4); }
    void drawIconInstrument(float x, float y, float s, uint32_t c) override { drawNative(x,y,s,s,73,0,0,c,c,4); }
    void drawIconMidi(float x, float y, float s, uint32_t c) override { drawNative(x,y,s,s,77,0,0,c,c,4); }
    void drawIconMic(float x, float y, float s, uint32_t c) override {
        drawCircle(x + s * 0.5f, y + s * 0.42f, s * 0.22f, c);
        drawLine(x + s * 0.28f, y + s * 0.62f, x + s * 0.72f, y + s * 0.62f, std::max(1.0f, s * 0.08f), c);
    }
    void drawIconDrums(float x, float y, float s, uint32_t c) override {
        drawCircle(x + s * 0.35f, y + s * 0.55f, s * 0.2f, c);
        drawCircle(x + s * 0.68f, y + s * 0.55f, s * 0.2f, c);
        drawLine(x + s * 0.2f, y + s * 0.25f, x + s * 0.8f, y + s * 0.25f,
                 std::max(1.0f, s * 0.08f), c);
    }
    void drawTriangle(float x1,float y1,float x2,float y2,float x3,float y3,float,uint32_t c) override { drawFilledTriangle(x1,y1,x2,y2,x3,y3,c); }
    void drawGoniometer(float x,float y,float w,float h,const float* l,const float* r,size_t n) override {
        if(!l||!r||n<2)return; const float cx=x+w*.5f,cy=y+h*.5f; drawLine(x,cy,x+w,cy,1,0x332f3a44u); drawLine(cx,y,cx,y+h,1,0x332f3a44u);
        const size_t first=n>256?n-256:0; for(size_t i=first+1;i<n;++i) drawLine(cx+l[i-1]*w*.45f,cy-r[i-1]*h*.45f,cx+l[i]*w*.45f,cy-r[i]*h*.45f,1,0xff48a8a0u);
    }
    std::string getBackendName() const override { return "Metal Studio Zero-Copy DMA"; }
    RenderingTelemetry getTelemetry() const override {
        return {0.0f, m_instanceCount, m_instanceCount * 4u, 0.0f};
    }

private:
    void appendNative(float x, float y, float w, float h, float p1, float p2, float p3,
                      uint32_t c1, uint32_t c2, float type) {
        if (m_instanceCount >= kMaxInstances || !m_mappedUniforms || !m_view) return;
        Uniforms& u = m_mappedUniforms[m_instanceCount++];
        const CGSize drawableSize = m_view.drawableSize;
        const float dw = drawableSize.width > 0.0 ? static_cast<float>(drawableSize.width) : 1.0f;
        const float dh = drawableSize.height > 0.0 ? static_cast<float>(drawableSize.height) : 1.0f;
        u.pos = { (x / dw) * 2.f, (y / dh) * 2.f, (w / dw) * 2.f, (h / dh) * 2.f };
        u.props = { p1, p2, p3, type };
        u.color1 = colorToFloat(c1);
        u.color2 = colorToFloat(c2);
    }

    simd_float4 colorToFloat(uint32_t c) { return {((c>>16)&0xFF)/255.f,((c>>8)&0xFF)/255.f,(c&0xFF)/255.f,((c>>24)&0xFF)/255.f}; }
    MTKView* m_view; id<MTLDevice> m_device; id<MTLCommandQueue> m_commandQueue;
    id<MTLCommandBuffer> m_commandBuffer; id<MTLRenderCommandEncoder> m_encoder;
    id<MTLRenderPipelineState> m_pipelineState; MTLRenderPassDescriptor* m_renderPassDescriptor;
    id<MTLBuffer> m_uniformBuffers[3]; int m_bufferIndex; uint32_t m_instanceCount;
    float m_scale; dispatch_semaphore_t m_semaphore; Uniforms* m_mappedUniforms;
    struct Scissor { float x, y, w, h; };
    std::array<Scissor, 16> m_scissors{};
    size_t m_scissorDepth = 0;
};
std::unique_ptr<IGraphicsKernel> GraphicsFactory::createDefault() { return std::make_unique<MetalKernel>(); }
}
