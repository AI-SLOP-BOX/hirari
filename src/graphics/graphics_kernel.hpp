#pragma once
#include <vector>
#include <string>
#include <memory>

namespace Aura::Graphics {

struct Vertex {
    float x, y;
    uint32_t color;
};

namespace Platform {

class IGraphicsKernel {
public:
    virtual ~IGraphicsKernel() = default;

    virtual bool initialize(void* nativeWindowHandle) = 0;
    virtual void beginFrame() = 0;
    virtual void endFrame() = 0;
    virtual void setScale(float s) = 0;
    virtual float getScale() const = 0;

    virtual void pushScissor(float x, float y, float w, float h) = 0;
    virtual void popScissor() = 0;

    virtual void drawWaveformPath(const float* minData, const float* maxData, size_t count, float x, float centerY, float w, float hScale, uint32_t color) = 0;
    virtual void drawVertexPath(const Vertex* vertices, size_t count, float thickness) = 0;
    virtual void drawVertexPathFilled(const Vertex* vertices, size_t count) = 0;
    virtual void updateSpectrogram(const std::vector<float>& data) = 0;

    virtual void drawMeter(uint32_t trackId, float levelL, float levelR, float x, float y, float w, float h) = 0;
    virtual void drawRoundedRect(float x, float y, float w, float h, float radius, uint32_t color) = 0;
    virtual void drawRect(float x, float y, float w, float h, uint32_t color) = 0;
    virtual void drawGradientRect(float x, float y, float w, float h, uint32_t colorTop, uint32_t colorBottom) = 0;
    virtual void drawText(const std::string& text, float x, float y, float size, uint32_t color) = 0;
    virtual float measureText(const std::string& text, float size) const = 0;

    virtual void drawCircle(float x, float y, float radius, uint32_t color) = 0;
    virtual void drawBrushedCircle(float x, float y, float radius, uint32_t color) = 0;
    virtual void drawLine(float x1, float y1, float x2, float y2, float thickness, uint32_t color) = 0;
    virtual void drawFilledPath(const std::vector<float>& points, uint32_t color) = 0;
    virtual void drawFilledTriangle(float x1, float y1, float x2, float y2, float x3, float y3, uint32_t color) = 0;
    virtual void drawDropShadow(float x, float y, float w, float h, float radius, uint32_t color) = 0;
    virtual void drawNeonRect(float x, float y, float w, float h, float radius, float glow, uint32_t color) = 0;
    virtual void drawGlassRect(float x, float y, float w, float h, float radius, uint32_t color) = 0;
    virtual void applyBlurEffect(float x, float y, float w, float h, float intensity) = 0;
    virtual void drawBezierPath(const std::vector<float>& points, uint32_t color, float thickness) = 0;
    virtual void drawBezierCurve(float x1, float y1, float cp1x, float cp1y, float cp2x, float cp2y, float x2, float y2, float thickness, uint32_t color) = 0;
    virtual void calculateBezier(float x1, float y1, float cp1x, float cp1y, float cp2x, float cp2y, float x2, float y2, float t, float& outX, float& outY) const = 0;
    virtual void drawArc(float cx, float cy, float radius, float startAngle, float endAngle, float thickness, uint32_t color) = 0;
    
    virtual void drawIconAudio(float x, float y, float s, uint32_t col) = 0;
    virtual void drawIconInstrument(float x, float y, float s, uint32_t col) = 0;
    virtual void drawIconMidi(float x, float y, float s, uint32_t col) = 0;
    virtual void drawIconMic(float x, float y, float s, uint32_t col) = 0;
    virtual void drawIconDrums(float x, float y, float s, uint32_t col) = 0;
    virtual void drawTriangle(float x1, float y1, float x2, float y2, float x3, float y3, float thickness, uint32_t color) = 0;
    virtual void drawGoniometer(float x, float y, float w, float h, const float* historyL, const float* historyR, size_t count) = 0;

    /**
     * @brief ACCELERATED ZERO-COPY PRIMITIVE: Direct DMA to Metal Shaders.
     */
    virtual void drawNative(float x, float y, float w, float h, float p1, float p2, float p3, uint32_t c1, uint32_t c2, float type) = 0;

    /**
     * @struct RenderingTelemetry
     * @brief Professional performance monitoring for the graphics engine.
     */
    struct RenderingTelemetry {
        float fps;
        uint32_t drawCallCount;
        uint32_t vertexCount;
        float gpuLoad; // If supported by backend
    };

    virtual RenderingTelemetry getTelemetry() const = 0;
    virtual std::string getBackendName() const = 0;
};

class GraphicsFactory {
public:
    static std::unique_ptr<IGraphicsKernel> createDefault();
};

} // namespace Platform
} // namespace Aura::Graphics
