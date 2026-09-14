import { forwardRef } from 'react';
import {
  ChibiPetCanvas,
  type ChibiPetCanvasHandle,
  type ChibiPetCanvasProps,
} from './ChibiPetCanvas';

export type ModelRendererHandle = ChibiPetCanvasHandle;
export type ModelCanvasProps = ChibiPetCanvasProps;

export const ModelCanvas = forwardRef<ModelRendererHandle, ModelCanvasProps>(
  (props, ref) => {
    return <ChibiPetCanvas ref={ref} {...props} />;
  }
);

ModelCanvas.displayName = 'ModelCanvas';
