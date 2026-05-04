"""Phase G.4 template registry.

Maps each ``visual_kind`` string the LLM emits (per
``backend/db/src/prompts/paragraph_visual_v1.md``) to its Manim
``Scene`` subclass. The G.5 sidecar reads this dict to look up the
right renderer per request.

The list of allowed kinds is defined as
``ALLOWED_VISUAL_KINDS`` in
``backend/api/src/animation/paragraphs.rs``; keep the two in sync.
"""

from __future__ import annotations

from typing import Type

from ._base import TemplateScene, render
from .axes_with_curve import AxesWithCurveScene
from .bar_chart import BarChartScene
from .equation_steps import EquationStepsScene
from .flow_chart import FlowChartScene
from .free_body import FreeBodyScene
from .function_plot import FunctionPlotScene
from .neural_net_layer import NeuralNetLayerScene
from .vector_field import VectorFieldScene

TEMPLATES: dict[str, Type[TemplateScene]] = {
    "function_plot": FunctionPlotScene,
    "axes_with_curve": AxesWithCurveScene,
    "vector_field": VectorFieldScene,
    "free_body": FreeBodyScene,
    "flow_chart": FlowChartScene,
    "bar_chart": BarChartScene,
    "equation_steps": EquationStepsScene,
    "neural_net_layer": NeuralNetLayerScene,
}

__all__ = [
    "TEMPLATES",
    "TemplateScene",
    "render",
    "AxesWithCurveScene",
    "BarChartScene",
    "EquationStepsScene",
    "FlowChartScene",
    "FreeBodyScene",
    "FunctionPlotScene",
    "NeuralNetLayerScene",
    "VectorFieldScene",
]
