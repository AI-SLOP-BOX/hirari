#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
FILES="$ROOT/src/core/aura_unified_engine_part_1.inc $ROOT/src/core/aura_unified_engine_part_2.inc $ROOT/src/core/aura_unified_engine_part_3.inc $ROOT/src/core/aura_unified_engine_part_4.inc $ROOT/src/core/aura_unified_engine_part_5.inc $ROOT/src/core/aura_unified_engine_part_6.inc $ROOT/src/core/aura_unified_engine_part_7.inc"

# These are deliberately process-wide, immutable/stateless or diagnostics-only.
# Project/audio state must use the owning AuraUnifiedEngine member instead.
ALLOW='AuraUnifiedEngine::getInstance|VideoEngine::getInstance|AudioDecoderManager::getInstance|ForensicJournaler::getInstance|EngineDiagnostics::getInstance|DiagnosticsKernel::getInstance|EngineOrchestrator::getInstance|TrigLUT512::getInstance|QualitativeMetricEngine::getInstance|LogBuffer::BlackBoxRegister::getInstance|AudioDecoderManager::getInstance'

violations=$(rg -n "(PDCManager|UndoTransactionManager|TempoMap|RoutingEngine|SidechainManager|BusSystem|MacroControlManager)::getInstance" $FILES || true)
if [ -n "$violations" ]; then
  echo "session-owned singleton references detected:"
  echo "$violations"
  exit 1
fi

echo "session singleton audit passed"
