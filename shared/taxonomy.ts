import type { Category } from './types';
import definitions from '../assets/taxonomy.json';
// Shared with Rust; definitions are Jev questions, never source-matching rules.
export const categories:Category[]=definitions.categories as Category[];
export const categoryById=Object.fromEntries(categories.map(c=>[c.id,c]));
export const coreQuestions=definitions.coreQuestions;
